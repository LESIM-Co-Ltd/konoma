//! A Markdown document turned into decorated lines and the block model, with no application
//! state: the one function konoma and the standalone app both call, so a document looks the same
//! in either. The host keeps what is its own (the cache, the change gutter, search matches, the
//! task write-back, the Tab items) and hands in the drawing settings and the picture lookups.

use std::collections::HashMap;

use konoma_widgets::markdown::{
    self, BlockAligns, CodeStyle, ImagePlacement, ImageSlot, LineOrigin, MathSlot, MdRenderExtras,
    MermaidSlot,
};
use ratatui::text::Line;

use crate::view::DocOptions;

/// The notice appended to a Markdown file cut at the display limit (its own paragraph; the host
/// writes it in this one language whatever the reader's).
pub const TRUNCATED_NOTE: &str = "\n\n— (省略: 表示上限に達しました) —";

/// The default open state of a `<details>` block from `md_details` and the tag's own `open`
/// attribute: `"open"`/`"closed"` force it, anything else honors the attribute the way GitHub does.
pub fn details_default_open(md_details: &str, open_attr: bool) -> bool {
    match md_details {
        "open" => true,
        "closed" => false,
        _ => open_attr,
    }
}

/// `DocOptions`' table and image alignment as the renderer's type.
pub fn block_aligns(o: &DocOptions) -> BlockAligns {
    use markdown::BlockAlign;
    let d = BlockAligns::default();
    BlockAligns {
        table: BlockAlign::from_config(&o.table_align, d.table),
        image: BlockAlign::from_config(&o.image_align, d.image),
    }
}

/// The drawing settings of one render.
pub struct RenderSettings<'a> {
    /// Display width in cells.
    pub width: u16,
    pub code: CodeStyle,
    /// The syntax theme of code blocks.
    pub code_theme: &'a str,
    pub icons: bool,
    /// The text under a mermaid picture.
    pub mermaid_caption: &'a str,
    /// Per `<details>` ordinal, the open state the user toggled (absent = the default).
    pub details_open: &'a HashMap<usize, bool>,
}

/// Where each picture of the document goes: the host's answers, asked while the document is
/// drawn. A closure may be called more than once for the same key (a table cell asks too).
pub struct PictureSlots<'a> {
    /// An image URL and the width budget of its cell (`None` = the page's).
    pub image: &'a dyn Fn(&str, Option<u16>) -> ImageSlot,
    /// A mermaid fence's code (the empty string asks whether fences are drawn as pictures).
    pub mermaid: &'a dyn Fn(&str) -> MermaidSlot,
    /// An expression and whether it is a display one.
    pub math: &'a dyn Fn(&str, bool) -> MathSlot,
    /// Fences are drawn as pictures (and so collected for rendering).
    pub mermaid_on: bool,
    /// Math is drawn as pictures.
    pub math_on: bool,
    /// Remote images can be shown (and so collected for fetching).
    pub remote_on: bool,
}

/// The decorated document.
pub struct RenderedDocument {
    pub lines: Vec<Line<'static>>,
    pub images: Vec<ImagePlacement>,
    /// Remote image URLs the document shows (empty unless `remote_on`).
    pub remote_urls: Vec<String>,
    pub mermaid_fences: Vec<String>,
    pub math_exprs: Vec<(String, bool)>,
    /// Lines of the text given in (before any preprocessing).
    pub src_lines: usize,
    /// Exactly the text the renderer parsed: front matter stripped, footnotes and inline HTML
    /// rewritten.
    pub pre_src: String,
    /// Per line of `pre_src`, the body line it came from.
    pub pre_origin: LineOrigin,
    /// The render's own record of the code blocks and checkboxes it drew.
    pub extras: MdRenderExtras,
    /// The open state of each `<details>` block, as drawn.
    pub details_states: Vec<bool>,
}

/// Render Markdown `text` the way a document is drawn: front matter, footnotes and inline HTML
/// per `opts`, then the block renderer. Sets the renderer's `<details>` state (thread-local) to
/// what it returns in `details_states`.
pub fn render_document(
    text: &str,
    opts: &DocOptions,
    set: &RenderSettings<'_>,
    slots: &PictureSlots<'_>,
) -> RenderedDocument {
    let src_lines = text.lines().count();
    // Front matter: split the leading `---`…`---` off so the body renders normally; a dim
    // metadata block is prepended to the result (and image line indices offset).
    let (fm_lines, src) = if opts.front_matter {
        match markdown::strip_front_matter(text) {
            (Some(fm), body) => (markdown::render_front_matter(&fm, set.width), body),
            (None, body) => (Vec::new(), body),
        }
    } else {
        (Vec::new(), text.to_string())
    };
    // Each pre-pass also reports, per output line, which body line it came from, so the
    // checkbox toggle can prove the checkbox it is about to edit really is the one on screen.
    let origin = markdown::identity_origin(&src);
    let (src, origin) = if opts.footnotes {
        markdown::process_footnotes_traced(&src, &origin)
    } else {
        (src, origin)
    };
    let (src, origin) = if opts.inline_html {
        markdown::process_inline_html_traced(&src, &origin)
    } else {
        (src, origin)
    };
    let pre_src = src.clone();
    let details_states: Vec<bool> = markdown::collect_details_open(&src)
        .iter()
        .enumerate()
        .map(|(ord, &attr)| {
            set.details_open
                .get(&ord)
                .copied()
                .unwrap_or_else(|| details_default_open(&opts.details, attr))
        })
        .collect();
    markdown::set_details_open(details_states.clone());
    let (mut lines, mut images, extras) = markdown::render_markdown_with_images_aligned(
        &src,
        set.width,
        set.code,
        set.code_theme,
        set.icons,
        &opts.task_states,
        slots.image,
        slots.mermaid,
        set.mermaid_caption,
        opts.alerts,
        slots.math,
        slots.math_on,
        block_aligns(opts),
    );
    let remote_urls = if slots.remote_on {
        markdown::collect_remote_image_urls(&src)
    } else {
        Vec::new()
    };
    let mermaid_fences = if slots.mermaid_on {
        markdown::collect_mermaid_fences(&src)
    } else {
        Vec::new()
    };
    let math_exprs = if slots.math_on {
        markdown::collect_math_exprs(&src)
    } else {
        Vec::new()
    };
    if !fm_lines.is_empty() {
        for p in &mut images {
            p.line += fm_lines.len();
        }
        let mut all = fm_lines;
        all.extend(lines);
        lines = all;
    }
    RenderedDocument {
        lines,
        images,
        remote_urls,
        mermaid_fences,
        math_exprs,
        src_lines,
        pre_src,
        pre_origin: origin,
        extras,
        details_states,
    }
}

/// The post-pass over a rendered document's lines: collapse the `label (URL)` links, auto-link
/// bare URLs and convert emoji shortcodes (per `opts`), then re-resolve the column of each
/// inline math picture against the lines as they now are (those passes change a line's width
/// left of the picture). Returns the lines and the link targets in order. Run it between
/// [`render_document`] and [`wrap_row_prefix`], so the rows are counted on what is drawn.
pub fn postprocess(
    lines: Vec<Line<'static>>,
    images: &mut [ImagePlacement],
    opts: &DocOptions,
    icons: bool,
) -> (Vec<Line<'static>>, Vec<String>) {
    let (lines, targets) = markdown::postprocess_lines(lines, icons, opts.autolink, opts.emoji);
    markdown::resolve_inline_math_cols(&lines, images);
    (lines, targets)
}

/// Prefix sums of the rows each line takes when wrapped at `width` (ratatui's own reflow):
/// `[0, rows of line 0, ...]`, so the first row of line `i` is element `i`.
pub fn wrap_row_prefix(lines: &[Line<'static>], width: u16) -> Vec<usize> {
    use ratatui::text::Text;
    use ratatui::widgets::{Paragraph, Wrap};
    let mut pre = Vec::with_capacity(lines.len() + 1);
    let mut acc = 0usize;
    pre.push(0);
    for line in lines {
        acc += Paragraph::new(Text::from(vec![line.clone()]))
            .wrap(Wrap { trim: false })
            .line_count(width)
            .max(1);
        pre.push(acc);
    }
    pre
}

#[cfg(test)]
mod tests;
