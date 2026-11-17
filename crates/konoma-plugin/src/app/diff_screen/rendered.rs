//! Standalone layout ownership. The worker, body adapter, frame and overlay are shared.
use std::ops::Range;

use super::*;
use crate::app::Laid;
use crate::document_render::{visible_line_range, PictureSlots, RenderSettings};
use crate::host::project::diff_blocks::{block_starts, DiffBlocks};
use crate::host::project::diff_frame::{
    clamp_decorated_scroll, draw_decorated_frame, DecoratedFrameValues, DecoratedScrollValues,
};
use crate::host::project::md_diff::{
    select_diff_baseline, MdDiffKind, MdDiffOutcome, MdDiffRequest,
};
use crate::host::project::rendered_diff::{
    render_diff_document, with_diff_gutter, RenderedDiffInput,
};
use konoma_host_core::project::{diff_representations, round_diff_view};
use konoma_widgets::markdown::{DiffMark, ImageSlot, MathSlot, MermaidSlot};

pub(super) struct RenderedCache {
    width: u16,
    rows: u16,
    epoch: Option<u64>,
    laid: Laid,
    marks: Vec<(Range<usize>, DiffMark)>,
}

impl RenderedCache {
    pub fn blocks(&self, viewport: u16) -> DiffBlocks {
        DiffBlocks {
            rows: block_starts(self.marks.iter().map(|(r, _)| r.clone()).collect())
                .into_iter()
                .map(|l| self.laid.first_row_of(l))
                .collect(),
            max_v: self.laid.rows().saturating_sub(viewport.into()),
        }
    }
}

impl DiffScreenState {
    /// Attach file resolution. The project caller executes the resulting Preview command.
    pub fn with_file_previews(
        mut self,
        source: crate::host::file_preview::FilePreviewSource,
    ) -> Self {
        self.file_previews = Some(source);
        self.resolve_preview();
        self.choose_effective();
        self
    }

    pub(super) fn resolve_preview(&mut self) {
        self.resolved_preview = None;
        let Some(source) = self.file_previews.clone() else {
            return;
        };
        let services = self.services.clone();
        let project_owned = self.project_owned;
        let request = self.request.clone();
        let (tx, rx) = mpsc::channel();
        #[cfg(any(test, feature = "test-hooks"))]
        let rx = if project_owned {
            super::super::project::idle_test_hooks::observe_replies(&*services, "resolve", rx)
        } else {
            rx
        };
        self.preview_reply = Some(rx);
        let _ = std::thread::Builder::new()
            .name("diff-preview-plan".into())
            .spawn(move || {
                let exists = request.path.is_file();
                // The project host classifies the live path; baseline sniffing belongs to
                // the media worker after an unavailable path has selected its diff.
                let baseline = (!exists && !project_owned)
                    .then(|| services.base_contents(&request.info, &request.path))
                    .flatten();
                let resolved = source.resolve(&request.path, baseline.as_deref());
                let _ = tx.send((resolved, exists));
            });
    }

    pub(super) fn target(&self) -> konoma_host_core::project::DiffTarget {
        if self.file_previews.is_some() {
            self.resolved_preview
                .as_ref()
                .map_or(konoma_host_core::project::DiffTarget::Other, |(r, _)| {
                    r.diff_target
                })
        } else {
            self.services.diff_target(&self.request.path, None)
        }
    }

    pub fn return_info(&self) -> DiffScreenReturn {
        DiffScreenReturn {
            path: self.request.path.clone(),
            choice: self.choice,
            effective: self.effective,
            layout: self.layout,
            scroll: self.scroll,
            hscroll: self.hscroll,
        }
    }

    pub fn take_notice(&mut self) -> Option<String> {
        self.notice.take()
    }

    pub(super) fn representations(&self) -> Vec<DiffView> {
        let target = self.target();
        let exists = self
            .resolved_preview
            .as_ref()
            .is_none_or(|(_, exists)| *exists);
        let mut reps = diff_representations(target, exists, self.landed_media());
        if self.file_previews.is_none() {
            reps.retain(|v| *v != DiffView::Preview);
        }
        reps
    }

    /// The target resolution of a file-preview source has not answered yet.
    fn answer_pending(&self) -> bool {
        self.file_previews.is_some() && self.resolved_preview.is_none()
    }

    pub(super) fn choose_effective(&mut self) {
        let effective = round_diff_view(self.choice, &self.representations());
        if effective == DiffView::Preview {
            self.command = Some(super::DiffScreenCommand::OpenPreview {
                path: self.request.path.clone(),
            });
            // Keep the executable diff visible until the worker has opened the file.
            return;
        }
        self.command = None;
        self.effective = effective;
        // The scroll to the first change is the landing of the diff's first load: only the first
        // answer of the target resolution reserves it (the host's rule, `diff_answer_awaited`).
        // An answer to a read made for a return keeps the view it was left at.
        let lands = self.effective == DiffView::Rendered && !self.media_active();
        if self.first_answer_awaited {
            self.pending_first_change = lands.then(|| self.request.path.clone());
            if self.file_previews.is_none() || self.resolved_preview.is_some() {
                self.first_answer_awaited = false;
            }
        } else if !lands && !self.answer_pending() {
            self.pending_first_change = None;
        }
        if self.effective == DiffView::Rendered
            && !self.media_active()
            && self
                .markdown
                .needs(&self.request.path, MdDiffKind::Rendered)
        {
            let result = self.markdown.kick(MdDiffRequest {
                gen: 0,
                path: self.request.path.clone(),
                root: self.request.root.clone(),
                kind: MdDiffKind::Rendered,
                baseline: select_diff_baseline(MdDiffKind::Rendered, false, false, false, || None),
                md_frontmatter: self.options.front_matter,
                md_footnotes: self.options.footnotes,
                md_inline_html: self.options.inline_html,
            });
            if let Some(result) = result {
                self.adopt_markdown(result);
            }
        }
        // Re-entering a previously unavailable result must adopt its effective fallback too.
        if self.effective == DiffView::Rendered
            && !self.media_active()
            && matches!(
                self.markdown
                    .landed_for(&self.request.path, MdDiffKind::Rendered),
                Some(MdDiffOutcome::Unavailable | MdDiffOutcome::NoBaseline)
            )
        {
            self.fallback();
        }
    }

    fn fallback(&mut self) {
        self.effective = DiffView::Source;
        self.pending_first_change = None;
        self.notice = Some(
            self.word(
                "rendered diff unavailable (falling back to source)",
                "整形diffを表示できません(ソースへ切替)",
            )
            .into(),
        );
    }

    pub(super) fn adopt_markdown(&mut self, result: MdDiffResult) -> bool {
        let on_screen = result.path == self.request.path && result.kind == MdDiffKind::Rendered;
        let unavailable = matches!(
            result.outcome,
            MdDiffOutcome::Unavailable | MdDiffOutcome::NoBaseline
        );
        let front_matter_only = matches!(
            result.outcome,
            MdDiffOutcome::Ready {
                front_matter_only: true,
                ..
            }
        );
        if !self.markdown.apply(result) {
            return false;
        }
        if on_screen {
            self.rendered = None;
            if self.effective == DiffView::Rendered {
                if unavailable {
                    self.fallback();
                } else if front_matter_only {
                    self.notice = Some(
                        self.word(
                            "only front matter changed (not shown here — press R to see it)",
                            "front matter だけの変更(ここには出ません・R で表示)",
                        )
                        .into(),
                    );
                }
            }
        }
        true
    }

    /// Fix the viewport, lay out, adopt completions, then consume a real result's reservation.
    /// Call outside drawing: completion effects must never change a flash during painting.
    pub fn prepare_frame(&mut self, area: Rect, mut pics: Option<&mut DocPics>) -> bool {
        let inner = Block::bordered().inner(area);
        self.viewport = inner.height;
        let media_changed = self.poll_media(pics.as_deref_mut());
        // The unresolved target is only a placeholder, not a binary classification.
        // Wait for the worker's plan before starting a representation for this path.
        if self.preview_reply.is_some() {
            return media_changed;
        }
        if self.media_active() || self.source_summary() {
            return self.prepare_media(inner, pics) || media_changed;
        }
        let mut changed = self.ensure_rendered(inner, pics.as_deref_mut()) || media_changed;
        let mut landed = false;
        while let Ok(result) = self.markdown_rx.try_recv() {
            landed |= self.adopt_markdown(result);
        }
        if landed {
            self.ensure_rendered(inner, pics);
        }
        changed |= landed;
        if self.effective == DiffView::Rendered {
            if self.rendered.is_some()
                && !self.markdown.pending_for(&self.request.path)
                && matches!(self.pending_first_change.take(), Some(p) if p == self.request.path)
            {
                if let Some(c) = &self.rendered {
                    if let Some((r, _)) = c.marks.first() {
                        self.scroll = c
                            .laid
                            .first_row_of(r.start)
                            .saturating_sub(3)
                            .min(u16::MAX as usize) as u16;
                    }
                }
            }
            if let Some(c) = &self.rendered {
                let update = clamp_decorated_scroll(DecoratedScrollValues {
                    vertical_limit: c.laid.rows().saturating_sub(inner.height.into()),
                    max_line_cols: c.laid.max_line_cols,
                    inner_width: inner.width,
                    viewport: inner.height,
                    scroll: self.scroll,
                    hscroll: self.hscroll,
                    wrap: self.display.wrap,
                });
                self.scroll = update.scroll;
                self.hscroll = update.hscroll;
            } else if self.pending.is_none() && !self.markdown.pending_for(&self.request.path) {
                // Nothing to lay out and nothing coming: a view of nothing starts at the top.
                // While the read for a return is still on its way the reader's position waits.
                self.scroll = 0;
                self.hscroll = 0;
            }
        }
        changed
    }

    fn ensure_rendered(&mut self, inner: Rect, pics: Option<&mut DocPics>) -> bool {
        if self.effective != DiffView::Rendered {
            return false;
        }
        let epoch = pics.as_deref().map(DocPics::epoch);
        if self
            .rendered
            .as_ref()
            .is_some_and(|c| c.width == inner.width && c.rows == inner.height && c.epoch == epoch)
        {
            return false;
        }
        let Some(MdDiffOutcome::Ready {
            old_pre,
            new_pre,
            ops,
            ..
        }) = self
            .markdown
            .landed_for(&self.request.path, MdDiffKind::Rendered)
        else {
            return false;
        };
        let input = RenderedDiffInput::new(&old_pre, &new_pre, &ops);
        let gutter = input.has_changes();
        let width = inner.width.saturating_sub(u16::from(gutter));
        let settings = RenderSettings {
            width,
            code: self.display.code_style(),
            code_theme: &self.display.theme.code_theme,
            icons: self.display.icons,
            mermaid_caption: self.word("Enter: full screen", "Enter: 全画面"),
            details_open: &Default::default(),
        };
        let body = match pics {
            Some(p) => p.render_diff(&input, &self.options, &settings, inner.height, self.lang),
            None => render_diff_document(
                &input,
                &self.options,
                &settings,
                &PictureSlots {
                    image: &|_, _| ImageSlot::Unavailable,
                    mermaid: &|_| MermaidSlot::Text,
                    math: &|_, _| MathSlot::Raw,
                    mermaid_on: false,
                    math_on: false,
                    remote_on: false,
                },
            ),
        };
        let prefix = body.row_prefix(self.display.wrap);
        let max_line_cols =
            body.lines.iter().map(|l| l.width()).max().unwrap_or(0) + usize::from(gutter);
        self.rendered = Some(RenderedCache {
            width: inner.width,
            rows: inner.height,
            epoch,
            laid: Laid {
                lines: body.lines,
                row_prefix: prefix,
                images: body.images,
                max_line_cols,
            },
            marks: body.diff_marks,
        });
        true
    }

    pub(super) fn draw_rendered(&self, buf: &mut Buffer, area: Rect) {
        let inner = Block::bordered().inner(area);
        let path = self
            .display
            .path_style
            .format(&self.request.root, &self.request.path);
        let title = DiffTitleValues {
            path: Some(&path),
            position: self
                .data
                .as_ref()
                .and_then(|d| changed_path_position(&d.paths, &self.request.path)),
            representation: Some("rendered"),
            ..Default::default()
        }
        .title(false);
        let (lines, local_scroll, total, max_line_cols) = match &self.rendered {
            Some(c) => {
                let (range, local) = visible_line_range(
                    c.laid.lines.len(),
                    &c.laid.row_prefix,
                    self.scroll.into(),
                    inner.height,
                );
                let start = range.start;
                (
                    with_diff_gutter(c.laid.lines[range].to_vec(), start, &c.marks),
                    local as u16,
                    c.laid.rows(),
                    c.laid.max_line_cols,
                )
            }
            None => (
                vec![ratatui::text::Line::from(
                    self.word("computing diff…", "差分を計算中…"),
                )],
                0,
                1,
                0,
            ),
        };
        let update = clamp_decorated_scroll(DecoratedScrollValues {
            vertical_limit: total.saturating_sub(inner.height.into()),
            max_line_cols,
            inner_width: inner.width,
            viewport: inner.height,
            scroll: self.scroll,
            hscroll: self.hscroll,
            wrap: self.display.wrap,
        });
        draw_decorated_frame(
            buf,
            area,
            DecoratedFrameValues {
                lines,
                local_scroll,
                hscroll: update.hscroll,
                wrap: self.display.wrap,
                extent: update.extent,
                title: &title,
                scroll_words: super::super::document::scroll_words(self.lang),
            },
        );
    }

    /// Paint after the shared frame. The ordinary overlay owns clipping and crop.
    pub fn draw_overlay(&self, pics: &mut DocPics, buf: &mut Buffer, area: Rect) {
        if self.help.is_some() && !self.project_owned {
            pics.draw(buf, area, None);
            return;
        }
        if self.media_active() {
            pics.draw_media(buf, Block::bordered().inner(area), &self.media.paints);
            return;
        }
        let doc = self
            .rendered
            .as_ref()
            .filter(|_| self.effective == DiffView::Rendered);
        pics.draw_with(
            buf,
            Block::bordered().inner(area),
            doc.map(|c| (&c.laid, self.scroll.into())),
            u16::from(doc.is_some_and(|c| !c.marks.is_empty())),
            None,
        );
    }
}
