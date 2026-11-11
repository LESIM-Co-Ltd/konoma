//! File opening belongs to a worker; presentation belongs to the stage 3 runner.
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, TryRecvError};

use super::{PreparedProjects, ProjectLoopCx};
use crate::app::diff_screen::DiffScreenReturn;
use crate::app::looping::FileLoopState;
use crate::app::{AppActs, AppCx, AppError, Opened, ReopenSource};
use crate::host::file_preview::{FilePreviewPlan, FilePreviewSource};
use crate::host::project::md_diff::{
    DiffBaseline, MdDiffKind, MdDiffOutcome, MdDiffRequest, MdDiffResult, MdDiffWorker,
};
use crate::host::project::preview_gutter::{
    first_change_position, line_gutter_marks, FirstChangePosition, GutterMark,
};
use crate::{Input, Manifest, OpenCx, Session, View, ViewCx, ViewKind, ViewReq};
use std::collections::HashMap;
use std::time::Duration;

#[cfg(test)]
#[path = "preview_worker_tests.rs"]
mod worker_tests;

/// One resolved declaration for the Preview's return key and all three hint surfaces.
pub(in crate::app) struct PreviewReturnHint {
    pub(in crate::app) keys: Vec<konoma_host_core::keymap::KeyPress>,
    pub(in crate::app) label: &'static str,
}

impl PreviewReturnHint {
    pub(in crate::app) fn new(
        keys: &konoma_host_core::keymap::CoreKeys,
        context: konoma_host_core::keymap::CoreContext,
        lang: konoma_host_core::lang::Lang,
    ) -> Self {
        use konoma_host_core::keymap::HostOp;
        let mut resolved: Vec<_> = keys
            .global
            .keys()
            .chain(keys.basic.keys())
            .chain(keys.contexts.values().flat_map(|t| t.keys()))
            .chain(keys.types.values().flat_map(|t| t.keys()))
            .copied()
            .filter(|k| {
                matches!(
                    keys.host_op(context, *k),
                    Some(HostOp::CycleRepresentation | HostOp::ImageReturnToDiff)
                )
            })
            .collect();
        resolved.sort_by_key(|k| konoma_host_core::help::display_order(*k));
        resolved.dedup();
        Self {
            keys: resolved,
            label: if lang == konoma_host_core::lang::Lang::Jp {
                "diff に戻る"
            } else {
                "back to diff"
            },
        }
    }

    pub(in crate::app) fn pairs(&self) -> Vec<(String, String)> {
        self.keys
            .iter()
            .map(|k| (konoma_host_core::help::help_key(*k), self.label.into()))
            .collect()
    }

    pub(in crate::app) fn footer(&self) -> String {
        format!(
            "{}:{}",
            self.keys
                .iter()
                .map(|k| konoma_host_core::help::footer_key(*k, false))
                .collect::<Vec<_>>()
                .join("/"),
            self.label
        )
    }
}

pub(super) struct FilePreviewState {
    pub(super) opened: Opened,
    pub(super) runner: FileLoopState,
    pub(super) source: DiffScreenReturn,
    pub(super) gutter: PreviewGutter,
}

/// One Preview owns its replies, deadline, layout input and path-scoped reservation.
/// Raw text reuses the opening diff's worker instead of selecting its baseline again.
pub(in crate::app) struct PreviewGutter {
    pub(in crate::app) project_owned: bool,
    path: PathBuf,
    root: PathBuf,
    enabled: bool,
    eligible: bool,
    pub(in crate::app) settings: crate::Settings,
    reservation: Option<PathBuf>,
    markdown: MdDiffWorker,
    markdown_rx: Receiver<MdDiffResult>,
    raw: Option<crate::app::diff_screen::FileDiffReply>,
    lines: Option<HashMap<u32, GutterMark>>,
    outcome: Option<MdDiffOutcome>,
    options: Option<crate::view::DocOptions>,
    pub(in crate::app) yield_on_timeout: bool,
    pub(in crate::app) owner_wake: Option<Duration>,
    #[cfg(any(test, feature = "test-hooks"))]
    pub(in crate::app) idle: Option<super::ProjectIdleSnapshot>,
    #[cfg(any(test, feature = "test-hooks"))]
    pub(in crate::app) detection: Option<crate::host::project::ProjectDetection>,
    #[cfg(any(test, feature = "test-hooks"))]
    pub(in crate::app) observed:
        Option<std::sync::Arc<std::sync::Mutex<Option<super::ProjectStateSnapshot>>>>,
    #[cfg(any(test, feature = "test-hooks"))]
    pub(in crate::app) probe: Option<super::ProjectLoopProbe>,
}

impl PreviewGutter {
    pub(super) fn retain_position(&mut self) {
        self.reservation = None;
    }
    #[cfg(any(test, feature = "test-hooks"))]
    pub(in crate::app) fn baseline_text_for_test(&self) -> Option<String> {
        match &self.outcome {
            Some(MdDiffOutcome::Ready { old_pre, .. }) => Some(old_pre.clone()),
            _ => None,
        }
    }

    fn new(
        path: PathBuf,
        root: PathBuf,
        enabled: bool,
        context: crate::host::project::DiffReadContext,
    ) -> Self {
        let (tx, markdown_rx) = mpsc::channel();
        let mut markdown = MdDiffWorker::default().with_read_context(context);
        markdown.attach(tx);
        Self {
            project_owned: false,
            reservation: Some(path.clone()),
            path,
            root,
            enabled,
            eligible: true,
            settings: crate::Settings::new(),
            markdown,
            markdown_rx,
            raw: None,
            lines: None,
            outcome: None,
            options: None,
            yield_on_timeout: false,
            owner_wake: None,
            #[cfg(any(test, feature = "test-hooks"))]
            idle: None,
            #[cfg(any(test, feature = "test-hooks"))]
            detection: None,
            #[cfg(any(test, feature = "test-hooks"))]
            observed: None,
            #[cfg(any(test, feature = "test-hooks"))]
            probe: None,
        }
    }

    pub(in crate::app) fn wake_in(&self) -> Option<Duration> {
        [
            (self.raw.is_some() || self.markdown.pending_for(&self.path))
                .then_some(Duration::from_millis(16)),
            self.owner_wake,
        ]
        .into_iter()
        .flatten()
        .min()
    }

    #[cfg(any(test, feature = "test-hooks"))]
    pub(in crate::app) fn settle_for_test(&mut self) -> bool {
        let mut changed = false;
        if let Some(raw) = self.raw.take() {
            let reply = raw
                .recv_timeout(Duration::from_secs(30))
                .expect("project idle fence: Preview raw document");
            self.lines =
                Some(reply.map_or_else(|_| HashMap::new(), |d| line_gutter_marks(&d.lines)));
            changed = true;
        }
        while self.markdown.pending_for(&self.path) {
            let reply = self
                .markdown_rx
                .recv_timeout(Duration::from_secs(30))
                .expect("project idle fence: Preview Markdown document");
            changed |= self.adopt(reply);
        }
        changed
    }

    fn adopt(&mut self, result: MdDiffResult) -> bool {
        if result.path != self.path
            || result.kind != MdDiffKind::Gutter
            || !self.markdown.apply(result)
        {
            return false;
        }
        self.outcome = self.markdown.landed_for(&self.path, MdDiffKind::Gutter);
        true
    }

    pub(in crate::app) fn poll(&mut self) -> bool {
        let mut changed = false;
        while let Ok(result) = self.markdown_rx.try_recv() {
            changed |= self.adopt(result);
        }
        if let Some(rx) = &self.raw {
            match rx.try_recv() {
                Ok(reply) => {
                    self.lines = Some(
                        reply.map_or_else(|_| HashMap::new(), |d| line_gutter_marks(&d.lines)),
                    );
                    self.raw = None;
                    changed = true;
                }
                Err(TryRecvError::Disconnected) => {
                    self.lines = Some(HashMap::new());
                    self.raw = None;
                    changed = true;
                }
                Err(TryRecvError::Empty) => {}
            }
        }
        changed
    }

    pub(in crate::app) fn prepare(&mut self, view: &View) {
        if !self.eligible {
            return;
        }
        match view {
            View::Document(dv)
                if matches!(&dv.origin, crate::input::Origin::File(p) if p == &self.path)
                    && !self.path.extension().is_some_and(|e| {
                        e.eq_ignore_ascii_case("mmd") || e.eq_ignore_ascii_case("mermaid")
                    }) =>
            {
                self.raw = None;
                if !self.enabled {
                    // The host has no Markdown marks or first-change row with the gutter off.
                    self.reservation = None;
                    return;
                }
                if self.options.as_ref().is_some_and(|o| o != &dv.options) {
                    self.markdown.invalidate();
                    self.outcome = None;
                }
                self.options = Some(dv.options.clone());
                if self.markdown.needs(&self.path, MdDiffKind::Gutter) {
                    // Always attach a reply channel: no baseline I/O on the UI thread.
                    let result = self.markdown.kick(MdDiffRequest {
                        gen: 0,
                        path: self.path.clone(),
                        root: self.root.clone(),
                        kind: MdDiffKind::Gutter,
                        baseline: DiffBaseline::Vcs,
                        md_frontmatter: dv.options.front_matter,
                        md_footnotes: dv.options.footnotes,
                        md_inline_html: dv.options.inline_html,
                    });
                    debug_assert!(result.is_none());
                }
            }
            View::Text(t)
                if t.file_lines
                    && match &t.source {
                        crate::view::TextSource::File(path) => path == &self.path,
                        _ => true,
                    } => {}
            _ => self.cancel(),
        }
    }

    pub(in crate::app) fn cancel(&mut self) {
        self.eligible = false;
        self.reservation = None;
        self.raw = None;
        self.lines = None;
        self.outcome = None;
        self.markdown.invalidate();
    }

    pub(in crate::app) fn document_gutter(
        &self,
    ) -> Option<crate::host::project::preview_gutter::FilePreviewGutter<'_>> {
        self.enabled
            .then(|| {
                crate::host::project::preview_gutter::FilePreviewGutter::from_outcome(
                    self.outcome.as_ref(),
                )
            })
            .flatten()
    }

    pub(in crate::app) fn line_marks(&self) -> Option<&HashMap<u32, GutterMark>> {
        self.enabled.then_some(self.lines.as_ref()).flatten()
    }

    pub(in crate::app) fn take_position(
        &mut self,
        document_row: Option<usize>,
        document: bool,
    ) -> Option<FirstChangePosition> {
        let ready = if document {
            self.outcome.is_some()
        } else {
            self.lines.is_some()
        };
        if !ready {
            return None;
        }
        let path = self.reservation.take()?;
        if path != self.path {
            return None;
        }
        let first = if document {
            document_row
        } else {
            self.lines
                .as_ref()?
                .keys()
                .min()
                .map(|n| (*n as usize).saturating_sub(1))
        };
        first.map(first_change_position)
    }
}

pub(super) struct PendingPreview {
    reply: Receiver<Result<Opened, AppError>>,
    source: DiffScreenReturn,
}

impl PendingPreview {
    #[cfg(any(test, feature = "test-hooks"))]
    pub(super) fn fence_for_test(&mut self) {
        let opened = self.wait_for_test();
        let (tx, rx) = mpsc::channel();
        tx.send(opened)
            .expect("project idle fence: Preview publication");
        self.reply = rx;
    }
    #[cfg(any(test, feature = "test-hooks"))]
    pub(super) fn wait_for_test(&self) -> Result<Opened, AppError> {
        self.reply
            .recv_timeout(Duration::from_secs(30))
            .expect("project idle fence: Preview open publication")
    }
    pub(super) fn matches(&self, diff: &DiffScreenReturn) -> bool {
        self.source.path == diff.path && diff.choice == konoma_host_core::project::DiffView::Preview
    }
    pub(super) fn start(prepared: &PreparedProjects, source: DiffScreenReturn) -> Option<Self> {
        let previews = prepared.file_previews.clone()?;
        let config = prepared.config.clone();
        let term = prepared.term;
        let lang = prepared.lang;
        let path = source.path.clone();
        #[cfg(test)]
        let fence = prepared.preview_open_fence.clone();
        let (tx, reply) = mpsc::channel();
        let _ = std::thread::Builder::new()
            .name("project-file-preview".into())
            .spawn(move || {
                let result = open(previews, path, &config, term, lang);
                let _ = tx.send(result);
                #[cfg(test)]
                if let Some(fence) = fence {
                    let _ = fence.send(());
                }
            });
        Some(Self { reply, source })
    }

    pub(super) fn poll(&self) -> Option<Result<Opened, AppError>> {
        match self.reply.try_recv() {
            Ok(result) => Some(result),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                Some(Err(AppError::Open("file diff worker disconnected".into())))
            }
        }
    }

    pub(super) fn finish(
        self,
        opened: Opened,
        prepared: &PreparedProjects,
        cx: &ProjectLoopCx,
        diff: &mut crate::app::diff_screen::DiffScreenState,
    ) -> FilePreviewState {
        let context = context(&opened, prepared, cx);
        let mut gutter = PreviewGutter::new(
            opened.path.clone(),
            diff.request.root.clone(),
            prepared.config.ui.git_gutter,
            diff.request.info.backend.diff_read_context(),
        );
        gutter.settings = crate::config::plugin_settings(
            &prepared.config.plugins,
            &prepared.config.ui.moved(),
            &opened.plugin_id,
        );
        gutter.yield_on_timeout = true;
        gutter.project_owned = diff.project_owned;
        #[cfg(any(test, feature = "test-hooks"))]
        {
            gutter.observed = prepared.test_observed.clone();
            gutter.probe = prepared.test_probe.clone();
            gutter.markdown_rx = super::idle_test_hooks::observe_replies(
                &*prepared.services,
                "preview",
                gutter.markdown_rx,
            );
        }
        let (raw, lines) = diff.take_preview_lines();
        gutter.raw = raw;
        gutter.lines = lines.map(|lines| line_gutter_marks(&lines));
        FilePreviewState {
            runner: FileLoopState::new(&context, &prepared.keys),
            opened,
            source: self.source,
            gutter,
        }
    }
}

impl FilePreviewState {
    pub(super) fn context(&self, prepared: &PreparedProjects, cx: &ProjectLoopCx) -> AppCx {
        context(&self.opened, prepared, cx)
    }
}

/// Enter a project Preview after publication, retaining the real project return and config.
#[cfg(any(test, feature = "test-hooks"))]
#[doc(hidden)]
pub fn run_project_preview_with_landed_gutter_for_test<B: ratatui::backend::Backend>(
    terminal: &mut ratatui::Terminal<B>,
    prepared: &mut PreparedProjects,
    source: FilePreviewSource,
    path: PathBuf,
    outcome: Option<MdDiffOutcome>,
) -> Result<ratatui::buffer::Buffer, AppError> {
    use crate::app::diff_screen::{DiffScreenState, FileDiffRequest};
    prepared.file_previews = Some(source);
    let info = prepared.service_info.values().next().unwrap().clone();
    let mut diff = DiffScreenState::open(
        FileDiffRequest {
            path: path.clone(),
            root: prepared.open_dir.clone(),
            info,
        },
        prepared.services.clone(),
        &prepared.config,
        prepared.settings.diff_layout,
        Some(konoma_host_core::project::DiffView::Preview),
    )
    .map_err(|e| AppError::Open(e.message.get(prepared.lang).into()))?;
    // The fixture service publishes raw content before returning its receiver.
    diff.poll()
        .map_err(|e| AppError::Open(e.message.get(prepared.lang).into()))?;
    let pending = PendingPreview::start(prepared, diff.return_info()).unwrap();
    let opened = pending
        .reply
        .recv_timeout(Duration::from_secs(10))
        .map_err(|e| AppError::Open(e.to_string()))??;
    let cx = ProjectLoopCx::default();
    let mut preview = pending.finish(opened, prepared, &cx, &mut diff);
    if let Some(outcome) = outcome {
        preview.gutter.adopt(MdDiffResult {
            gen: 0,
            path,
            kind: MdDiffKind::Gutter,
            outcome,
        });
    }
    prepared.diff_return = prepared.screen.take();
    prepared.diff = Some(diff);
    prepared.file_preview = Some(preview);
    super::run_project_with(
        terminal,
        prepared,
        &mut Vec::<crate::app::AppEvent>::new().into_iter(),
        &cx,
        None,
    )?;
    Ok(crate::app::draw::test_frame())
}

/// Seed a published reply, then exercise the actual standalone opener and file runner.
#[cfg(any(test, feature = "test-hooks"))]
#[doc(hidden)]
#[allow(clippy::too_many_arguments)]
pub fn run_preview_with_landed_gutter_for_test<B: ratatui::backend::Backend>(
    terminal: &mut ratatui::Terminal<B>,
    source: FilePreviewSource,
    path: PathBuf,
    root: PathBuf,
    config: &konoma_host_core::shared::SharedConfig,
    outcome: Option<MdDiffOutcome>,
    lines: Vec<konoma_vcs_core::git::DiffLine>,
    events: Vec<crate::app::AppEvent>,
) -> Result<ratatui::buffer::Buffer, AppError> {
    let term = crate::TermCaps::new();
    let mut opened = open(source, path.clone(), config, term, "en")?;
    let cx = AppCx {
        title: opened.title.clone(),
        path: path.clone(),
        plugin_id: opened.plugin_id.clone(),
        shows: opened.shows.clone(),
        open_dir: Some(root.clone()),
        display: crate::app::Display::of(config),
        ..Default::default()
    };
    let keys = crate::app::AppKeys::defaults();
    let mut runner = FileLoopState::new(&cx, &keys);
    let mut gutter =
        PreviewGutter::new(path.clone(), root, config.ui.git_gutter, Default::default());
    gutter.settings =
        crate::config::plugin_settings(&config.plugins, &config.ui.moved(), &opened.plugin_id);
    gutter.lines = Some(line_gutter_marks(&lines));
    if let Some(outcome) = outcome {
        *gutter.markdown.landed_for_test() = Some((path, MdDiffKind::Gutter, 0, outcome.clone()));
        gutter.outcome = Some(outcome);
    }
    crate::app::looping::run_file_with_preview(
        terminal,
        &mut *opened.session,
        &mut events.into_iter(),
        &cx,
        &mut runner,
        None,
        Some(&mut gutter),
    )
    .map(|_| crate::app::draw::test_frame())
}

fn context(opened: &Opened, prepared: &PreparedProjects, cx: &ProjectLoopCx) -> AppCx {
    let config = prepared.config.clone();
    let settings =
        move |id: &str| crate::config::plugin_settings(&config.plugins, &config.ui.moved(), id);
    let acts_config = prepared.config.clone();
    let mut reopen = ReopenSource::from_registry(opened.registry.clone(), Box::new(settings))
        .with_acts(move |m| {
            AppActs::from_config(&m.id, m, &acts_config.keys, &acts_config.plugins).0
        });
    if let Some(plugin) = opened.registry.plugin(&opened.plugin_id) {
        reopen = reopen.with_file(
            plugin,
            Input::File {
                path: opened.path.clone(),
                hint: opened.hint.clone(),
            },
            crate::config::plugin_settings(
                &prepared.config.plugins,
                &prepared.config.ui.moved(),
                &opened.plugin_id,
            ),
        );
    }
    let mut launch = cx.launch.clone();
    if let Some(launch) = &mut launch {
        launch.verbs = opened.manifest.verbs.clone();
    }
    AppCx {
        lang: prepared.lang.into(),
        term: prepared.term,
        title: opened.title.clone(),
        unavailable: opened.unavailable.clone(),
        plugin_id: opened.plugin_id.clone(),
        path: opened.path.clone(),
        hint: opened.hint.clone(),
        shows: opened.shows.clone(),
        reopen: Some(reopen),
        display: prepared.display.clone(),
        open_dir: Some(prepared.open_dir.clone()),
        acts: AppActs::from_config(
            &opened.plugin_id,
            &opened.manifest,
            &prepared.config.keys,
            &prepared.config.plugins,
        )
        .0,
        launch,
        converts: opened.manifest.converts,
    }
}

fn open(
    source: FilePreviewSource,
    path: PathBuf,
    config: &konoma_host_core::shared::SharedConfig,
    term: crate::TermCaps,
    lang: &str,
) -> Result<Opened, AppError> {
    let metadata = std::fs::metadata(&path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            AppError::NoFile(path.clone())
        } else {
            AppError::Unreadable(path.clone(), e)
        }
    })?;
    if !metadata.is_file() {
        return Err(AppError::NotAFile(path));
    }
    let registry = source.registry();
    let (manifest, hint, session): (_, _, Box<dyn Session>) = match source.plan(&path, None) {
        FilePreviewPlan::Plugin { id, hint } => {
            let plugin = registry
                .plugin(&id)
                .ok_or_else(|| AppError::Plugin(format!("plugin `{id}` is not registered")))?;
            let manifest = registry
                .manifest(&id)
                .cloned()
                .unwrap_or_else(|| plugin.manifest());
            let settings = crate::config::plugin_settings(&config.plugins, &config.ui.moved(), &id);
            let session = plugin
                .open(
                    Input::File {
                        path: path.clone(),
                        hint: hint.clone(),
                    },
                    &OpenCx::new(&settings, lang).with_term(term),
                )
                .map_err(|f| AppError::Open(f.message.get(lang).into()))?;
            (manifest, hint, session)
        }
        FilePreviewPlan::Text => {
            let bytes = std::fs::read(&path).map_err(|e| AppError::Unreadable(path.clone(), e))?;
            (
                Manifest::new("text", "0").shows(ViewKind::Text).file_text(),
                None,
                Box::new(Text(String::from_utf8_lossy(&bytes).into_owned())),
            )
        }
        _ => {
            return Err(AppError::NotHandled {
                path,
                why: "no plugin is available to show this document".into(),
            })
        }
    };
    Ok(Opened {
        title: path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        plugin_id: manifest.id.clone(),
        unavailable: manifest.unavailable.clone(),
        shows: manifest.shows.clone(),
        manifest,
        hint,
        path,
        session,
        registry,
    })
}

struct Text(String);
impl Session for Text {
    fn view(&mut self, _: &ViewReq, _: &ViewCx) -> Result<View, crate::Failure> {
        Ok(View::Text(crate::view::TextView {
            source: crate::view::TextSource::Owned(self.0.clone()),
            styler: None,
            file_lines: true,
            syntax_name: None,
        }))
    }
}
