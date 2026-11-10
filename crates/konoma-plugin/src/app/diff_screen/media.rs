//! Media ownership only: computation, drawing and cache conversion belong to shared modules.
use super::*;
use crate::host::project::md_diff::{select_diff_baseline, MdDiffKind};
use crate::host::project::media_diff::{self as shared, *};
use konoma_host_core::project::DiffTarget;
use konoma_widgets::media_diff::MediaDiffLayout;
use konoma_widgets::picture_overlay::PicturePaintRequest;

pub(super) struct Media {
    pub worker: MediaDiffWorker,
    rx: mpsc::Receiver<MediaDiffResult>,
    pub page: u32,
    pub layout: MediaDiffLayout,
    outcome: Option<MediaDiffOutcome>,
    cell_px: Option<(u32, u32)>,
    pub paints: Vec<(String, PicturePaintRequest)>,
}

impl Media {
    pub fn new(layout: &str) -> Self {
        let (tx, rx) = mpsc::channel();
        let mut worker = MediaDiffWorker::default();
        worker.attach(tx);
        Self {
            worker,
            rx,
            page: 1,
            layout: MediaDiffLayout::parse(layout),
            outcome: None,
            cell_px: None,
            paints: vec![],
        }
    }
    pub fn pending(&self) -> bool {
        self.worker.busy() || self.worker.queued().is_some()
    }
}

impl DiffScreenState {
    #[cfg(any(test, feature = "test-hooks"))]
    pub(super) fn settle_media_for_test(&mut self, mut pictures: Option<&mut DocPics>) {
        while self.media.pending() {
            self.poll_media(pictures.as_deref_mut());
            if self.media.worker.busy() {
                let result = self
                    .media
                    .rx
                    .recv_timeout(std::time::Duration::from_secs(30))
                    .expect("project idle fence: shown target media");
                self.adopt_media(result, &mut pictures);
            }
        }
    }
    /// Supply an attached worker and receiver, including a host's publication fence.
    pub fn with_media_worker(
        mut self,
        worker: MediaDiffWorker,
        rx: mpsc::Receiver<MediaDiffResult>,
    ) -> Self {
        self.media.worker = worker;
        self.media.rx = rx;
        self
    }

    pub fn media_page(&self) -> u32 {
        self.media.page.max(1)
    }
    pub fn media_layout(&self) -> MediaDiffLayout {
        self.media.layout
    }

    pub(super) fn media_active(&self) -> bool {
        self.effective == DiffView::Rendered && self.target() != DiffTarget::Markdown
    }
    fn media_outcome(&self) -> Option<&MediaDiffOutcome> {
        self.media.worker.outcome_for(&self.request.path)
    }
    pub(super) fn landed_media(&self) -> Option<konoma_host_core::project::LandedMedia> {
        use konoma_host_core::project::LandedMedia;
        self.media_outcome().map(|outcome| match outcome {
            MediaDiffOutcome::Ready {
                kind: MediaDiffKind::Svg,
                ..
            } => LandedMedia::Svg,
            MediaDiffOutcome::Ready { .. } => LandedMedia::Picture,
            MediaDiffOutcome::Summary { .. } | MediaDiffOutcome::Unavailable => {
                LandedMedia::NotPicture
            }
        })
    }
    pub(super) fn media_can_page(&self) -> bool {
        self.media_active() && shared::can_page(self.media_outcome())
    }
    pub(super) fn media_showing_pictures(&self) -> bool {
        self.media_active() && shared::showing_pictures(self.media_outcome())
    }
    pub(super) fn turn_media_page(&mut self, dir: i32) {
        if self.media_active() {
            self.media.page = shared::page_turn(self.media_outcome(), self.media_page(), dir);
        }
    }

    fn dispatch_media(&mut self, want: MediaDiffWant, pics: &mut Option<&mut DocPics>) -> bool {
        self.media.worker = std::mem::take(&mut self.media.worker)
            .with_read_context(self.request.info.backend.diff_read_context());
        let (path, page, raster_px) = want;
        let services = self.services.clone();
        let source = self.file_previews.clone();
        let req = MediaDiffRequest {
            gen: 0,
            path,
            root: self.request.root.clone(),
            page,
            raster_px,
            baseline: select_diff_baseline(MdDiffKind::Rendered, false, false, false, || None),
            classify: Arc::new(move |path, bytes| match &source {
                Some(source) => source.resolve(path, bytes).diff_target,
                None => services.diff_target(path, bytes),
            }),
            rasterize_svg,
            cancel: Arc::default(),
        };
        if let Some(result) = self.media.worker.dispatch(req) {
            self.adopt_media(result, pics)
        } else {
            false
        }
    }

    fn adopt_media(&mut self, result: MediaDiffResult, pics: &mut Option<&mut DocPics>) -> bool {
        self.media
            .worker
            .apply(result, |computed| match pics.as_deref_mut() {
                Some(p) => p.materialize_media_diff(computed),
                None => shared::materialize_media_diff(computed, |_| {}),
            })
    }

    pub(super) fn poll_media(&mut self, mut pics: Option<&mut DocPics>) -> bool {
        let mut changed = false;
        while let Ok(result) = self.media.rx.try_recv() {
            changed |= self.adopt_media(result, &mut pics);
        }
        let next = (!self.media.worker.busy())
            .then(|| self.media.worker.take_queued())
            .flatten();
        if let Some(next) = next {
            changed |= self.dispatch_media(next, &mut pics);
        }
        changed
    }

    pub(super) fn prepare_media(&mut self, inner: Rect, mut pics: Option<&mut DocPics>) -> bool {
        self.media.cell_px = pics.as_deref().map(DocPics::cell_px);
        let mut changed = false;
        let kind = match self.media_outcome() {
            Some(MediaDiffOutcome::Ready { kind, .. }) => Some(*kind),
            _ => shared::classify_kind_with(
                self.services.diff_target(&self.request.path, None),
                &self.request.path,
                Some(&[]),
            ),
        };
        let px = normalize_raster_px(
            kind,
            self.media
                .cell_px
                .map_or((64, 64), |cell| media_raster_box(inner, cell)),
        );
        let path = self.request.path.clone();
        let page = self.media_page();
        if self.media.worker.needs(&path, page, px) {
            if let Some(want) = self.media.worker.kick((path.clone(), page, px)) {
                changed |= self.dispatch_media(want, &mut pics);
            }
        }
        let mut outcome = self.media.worker.landed_for(&path, page, px);
        if let (Some(p), Some(outcome)) = (pics.as_deref(), outcome.as_ref()) {
            if live_cache_keys(outcome)
                .iter()
                .any(|key| !p.contains_media_key(key))
            {
                self.media.worker.invalidate();
                if let Some(want) = self.media.worker.kick((path.clone(), page, px)) {
                    changed |= self.dispatch_media(want, &mut pics);
                }
            }
        }
        outcome = self.media.worker.landed_for(&path, page, px);
        self.media.outcome = outcome;
        self.scroll = 0;
        self.hscroll = 0;
        changed
    }

    pub(super) fn media_layout_label(&self, layout: MediaDiffLayout) -> &str {
        match layout {
            MediaDiffLayout::Auto => self.word("auto", "自動"),
            MediaDiffLayout::Side => self.word("side by side", "左右"),
            MediaDiffLayout::Stack => self.word("stacked", "上下"),
        }
    }

    pub(super) fn draw_media(&mut self, buf: &mut Buffer, area: Rect) {
        let state = media_diff_draw_state(
            self.media.outcome.as_ref(),
            self.media.cell_px.is_some(),
            MediaDiffStateWords {
                computing: self.word("computing diff…", "差分を計算中…"),
                no_changes: self.word("(no changes)", "(変更なし)"),
                summary: crate::host::project::diff_frame::BinarySummaryWords {
                    binary_file: self.word("binary file", "バイナリファイル"),
                    none: self.word("(none)", "(なし)"),
                    same_size_different_content: self
                        .word("same size, content differs", "同じサイズ・内容が異なる"),
                    byte_unit: self.word("B", "バイト"),
                },
            },
            konoma_base::size::human_size,
        );
        let path = self
            .display
            .path_style
            .format(&self.request.root, &self.request.path);
        let base = match &self.media.outcome {
            Some(MediaDiffOutcome::Ready { base, .. }) => *base,
            _ => MediaBase::Head,
        };
        let old_label = match base {
            MediaBase::Head => "HEAD",
            MediaBase::JjParent => self.word("parent commit (@-)", "親コミット (@-)"),
            MediaBase::FollowStart => self.word("follow start", "フォロー開始時点"),
        };
        let new_label = if self.request.info.backend == super::super::project::ProjectBackend::Jj {
            self.word("working copy (@)", "作業コピー (@)")
        } else {
            self.word("working tree", "作業ツリー")
        };
        let mut paints = Vec::new();
        draw_media_diff(
            buf,
            area,
            MediaDiffDrawValues {
                state: &state,
                title: DiffTitleValues {
                    path: Some(&path),
                    position: self
                        .data
                        .as_ref()
                        .and_then(|d| changed_path_position(&d.paths, &self.request.path)),
                    representation: Some("side by side"),
                    ..Default::default()
                },
                cell_px: self.media.cell_px.unwrap_or((1, 1)),
                layout: self.media.layout,
                page: self.media_page(),
                old_label,
                new_label,
                words: MediaDiffWords {
                    before: self.word("Before", "変更前"),
                    after: self.word("After", "変更後"),
                    identical: self.word("identical content", "内容は同一"),
                    absent_old: self.word(
                        "new file (no old version)",
                        "新規ファイル（変更前はありません）",
                    ),
                    absent_new: self.word(
                        "deleted (no new version)",
                        "削除されました（変更後はありません）",
                    ),
                    cannot_display: self.word("cannot display", "表示できません"),
                    page_missing: self.word("this page doesn't exist", "このページはありません"),
                },
                format_size: konoma_base::size::human_size,
            },
            |key, request, _| {
                paints.push((key.to_owned(), request));
                false
            },
        );
        self.media.paints = paints;
    }
}

fn rasterize_svg(
    data: &[u8],
    path: &std::path::Path,
    max: u32,
    cancelled: &dyn Fn() -> bool,
) -> Result<image::DynamicImage, konoma_svg::svg::RasterError> {
    use crate::canvas_pictures::SvgDraw;
    super::super::InProcessSvg.untrusted(data, path, None, max, cancelled)
}
