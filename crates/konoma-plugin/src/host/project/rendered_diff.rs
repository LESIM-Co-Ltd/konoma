//! Rendered Markdown diff bodies, independent of cache ownership and picture resolution.
use std::ops::Range;

use konoma_widgets::markdown::{self, model::Doc, BlockOp, DiffMark, ImagePlacement};
use ratatui::text::Line;

use crate::document_render::{
    block_aligns, postprocess, visible_line_range, wrap_row_prefix, PictureRequests, PictureSlots,
    RenderSettings,
};
use crate::view::DocOptions;

/// The final gutter pass on a visible logical slice. Removed blocks keep their dim spans.
/// An unchanged document gains no gutter column.
pub fn with_diff_gutter(
    lines: Vec<Line<'static>>,
    lo: usize,
    marks: &[(Range<usize>, DiffMark)],
) -> Vec<Line<'static>> {
    use ratatui::style::{Color, Modifier, Style};
    use ratatui::text::Span;
    if marks.is_empty() {
        return lines;
    }
    lines
        .into_iter()
        .enumerate()
        .map(|(i, line)| {
            let row = lo + i;
            let mark = marks
                .iter()
                .find(|(range, _)| range.contains(&row))
                .map(|(_, m)| *m);
            let (glyph, color) = match mark {
                Some(DiffMark::Added) => ("▌", Color::Rgb(87, 171, 90)),
                Some(DiffMark::Modified) => ("▌", Color::Rgb(216, 166, 74)),
                Some(DiffMark::Removed) => ("▌", Color::Rgb(199, 84, 80)),
                None => (" ", Color::Reset),
            };
            let style = line.style;
            let mut spans: Vec<Span<'static>> = Vec::with_capacity(line.spans.len() + 1);
            spans.push(Span::styled(glyph, Style::new().fg(color)));
            if mark == Some(DiffMark::Removed) {
                spans.extend(line.spans.into_iter().map(|s| {
                    let st = s.style.add_modifier(Modifier::DIM);
                    Span::styled(s.content, st)
                }));
            } else {
                spans.extend(line.spans);
            }
            Line::from(spans).style(style)
        })
        .collect()
}

/// The worker's capped, preprocessed text and its already-computed block alignment.
/// Parsing borrows that text; neither preprocessing nor block alignment is repeated here.
pub struct RenderedDiffInput<'a> {
    pub old_pre: &'a str,
    pub new_pre: &'a str,
    pub old: Doc<'a>,
    pub new: Doc<'a>,
    pub ops: &'a [BlockOp],
}

impl<'a> RenderedDiffInput<'a> {
    pub fn new(old_pre: &'a str, new_pre: &'a str, ops: &'a [BlockOp]) -> Self {
        Self {
            old_pre,
            new_pre,
            old: Doc::parse(old_pre),
            new: Doc::parse(new_pre),
            ops,
        }
    }

    /// Decide the gutter before rendering at the resulting width.
    pub fn has_changes(&self) -> bool {
        markdown::ops_has_any_change(self.ops)
    }
}

/// Postprocessed lines, placements and requests from both versions, in old-then-new order.
/// No deduplication: the picture request loop already handles cached and in-flight keys.
pub struct RenderedDiffBody {
    pub lines: Vec<Line<'static>>,
    pub images: Vec<ImagePlacement>,
    pub diff_marks: Vec<(Range<usize>, DiffMark)>,
    pub remote_urls: Vec<String>,
    pub mermaid_fences: Vec<String>,
    pub math_exprs: Vec<(String, bool)>,
    pub src_lines: usize,
    pub pre_src: String,
    width: u16,
}

impl RenderedDiffBody {
    pub fn requests(&self) -> PictureRequests<'_> {
        PictureRequests {
            remote_urls: &self.remote_urls,
            mermaid_fences: &self.mermaid_fences,
            math_exprs: &self.math_exprs,
        }
    }

    /// Count the rows of the final lines, at the body width (excluding any change gutter).
    pub fn row_prefix(&self, wrap: bool) -> Vec<usize> {
        if wrap && self.width > 0 {
            wrap_row_prefix(&self.lines, self.width)
        } else {
            Vec::new()
        }
    }

    /// Clone only visible lines, using the prefix the host computed when it cached this body.
    /// Search, focus and the final gutter pass remain the caller's presentation state.
    pub fn visible_slice(
        &self,
        prefix: &[usize],
        scroll: u16,
        height: u16,
    ) -> (Vec<Line<'static>>, u16) {
        let (range, local) = visible_line_range(self.lines.len(), prefix, scroll.into(), height);
        (self.lines[range].to_vec(), local as u16)
    }
}

/// Draw the worker's aligned parts with the same settings and slots as an ordinary document.
/// Diff details always honor their own attributes; ordinary-preview overrides do not apply.
/// The display cap and Markdown prepasses belong to the worker, before this adapter.
pub fn render_diff_document(
    input: &RenderedDiffInput<'_>,
    options: &DocOptions,
    settings: &RenderSettings<'_>,
    slots: &PictureSlots<'_>,
) -> RenderedDiffBody {
    markdown::set_details_open(Vec::new());
    let (lines, mut images, _, diff_marks) = markdown::render_markdown_diff_from_parts(
        &input.old,
        input.old_pre,
        &input.new,
        input.new_pre,
        input.ops,
        settings.width,
        settings.code,
        settings.code_theme,
        settings.icons,
        &options.task_states,
        slots.image,
        slots.mermaid,
        settings.mermaid_caption,
        options.alerts,
        slots.math,
        slots.math_on,
        block_aligns(options),
    );
    let (lines, _) = postprocess(lines, &mut images, options, settings.icons);
    let both = |collect: fn(&str) -> Vec<String>, enabled| {
        if enabled {
            let mut requests = collect(input.old_pre);
            requests.extend(collect(input.new_pre));
            requests
        } else {
            Vec::new()
        }
    };
    let math_exprs = if slots.math_on {
        let mut requests = markdown::collect_math_exprs(input.old_pre);
        requests.extend(markdown::collect_math_exprs(input.new_pre));
        requests
    } else {
        Vec::new()
    };
    RenderedDiffBody {
        lines,
        images,
        diff_marks,
        remote_urls: both(markdown::collect_remote_image_urls, slots.remote_on),
        mermaid_fences: both(markdown::collect_mermaid_fences, slots.mermaid_on),
        math_exprs,
        src_lines: input.new_pre.lines().count(),
        pre_src: input.new_pre.to_string(),
        width: settings.width,
    }
}
