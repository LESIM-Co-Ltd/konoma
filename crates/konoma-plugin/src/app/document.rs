//! A Markdown document (`View::Document`) in the standalone app.
//!
//! The page is laid out by the one shared pipeline the konoma host uses
//! ([`crate::document_render`]): [`render_document`], then [`postprocess`], then
//! [`wrap_row_prefix`] when the text wraps (`ui.wrap`; off, it scrolls sideways). It is
//! drawn the way the host draws a decorated document: a bordered frame, the logical lines in the
//! visible range, soft-wrapped by ratatui and scrolled in display rows.
//!
//! Pictures: with a [`DocPics`] (a terminal that draws pictures) the page reserves cells for
//! them with the sizes the picture cache knows and asks for what is missing; the overlay is drawn
//! by [`DocPics::draw`]. Without one every picture is text (`ImageSlot::Unavailable`,
//! `MermaidSlot::Text`, `MathSlot::Raw`), which is what the host does when its terminal cannot
//! draw pictures.

use std::collections::HashMap;
use std::sync::Arc;

use konoma_host_core::help_msg::CoreMsg;
use konoma_host_core::lang::Lang;
use konoma_widgets::markdown::{ImagePlacement, ImageSlot, MathSlot, MermaidSlot};
use konoma_widgets::scroll::{render_scrollbar, scroll_title, ScrollExtent, ScrollWords};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::{Line as RLine, Text};
use ratatui::widgets::{Block, Paragraph, Widget, Wrap};

use super::display::Display;
use super::doc_pictures::DocPics;
use crate::document_render::{
    postprocess, render_document, visible_line_range, wrap_row_prefix, PictureSlots,
    RenderSettings, TRUNCATED_NOTE,
};
use crate::host::project::preview_gutter::{
    with_line_gutter, with_preview_gutter, FilePreviewGutter, GutterMark,
};
use crate::input::Origin;
use crate::view::{DocOptions, DocumentView};

/// Final text/code pass, after the caret and any line numbers have been drawn.
pub(in crate::app) fn lay_out_lines(
    lines: Vec<RLine<'static>>,
    first_line: usize,
    marks: Option<&HashMap<u32, GutterMark>>,
) -> Vec<RLine<'static>> {
    match marks {
        Some(marks) => with_line_gutter(lines, first_line, marks),
        None => lines,
    }
}

/// A document laid out at one width.
pub struct Laid {
    pub lines: Vec<RLine<'static>>,
    /// The first display row of each logical line, and the total at the end.
    pub row_prefix: Vec<usize>,
    /// The pictures the page reserves cells for (`line` is a logical line of `lines`).
    pub images: Vec<ImagePlacement>,
    /// The widest line in cells (the bound of the sideways scroll).
    pub max_line_cols: usize,
}

impl Laid {
    /// The first display row of logical line `line`. Without wrapping (no prefix sums) a line is
    /// a row.
    pub fn first_row_of(&self, line: usize) -> usize {
        if self.row_prefix.is_empty() {
            return line;
        }
        self.row_prefix.get(line).copied().unwrap_or(0)
    }

    /// How many display rows the document takes.
    pub fn rows(&self) -> usize {
        match self.row_prefix.last() {
            Some(n) => *n,
            None => self.lines.len(),
        }
    }
}

/// The text under a mermaid picture (the host's words).
fn mermaid_caption(lang: Lang) -> &'static str {
    match lang {
        Lang::Jp => "Enter: 全画面",
        Lang::En => "Enter: full screen",
    }
}

/// Lay `dv` out at `width` cells in a viewport of `rows` rows. `pics` is the picture state of a
/// terminal that draws pictures (`None`: every picture is text).
pub fn lay_out(
    dv: &DocumentView,
    width: u16,
    rows: u16,
    lang: Lang,
    pics: Option<&mut DocPics>,
    display: &Display,
) -> Laid {
    lay_out_with_gutter(dv, width, rows, lang, pics, display, None)
}

/// Only an ordinary Markdown file accepts the worker's landed file alignment.
#[allow(clippy::too_many_arguments)]
pub(in crate::app) fn lay_out_with_gutter(
    dv: &DocumentView,
    width: u16,
    rows: u16,
    lang: Lang,
    pics: Option<&mut DocPics>,
    display: &Display,
    gutter: Option<FilePreviewGutter<'_>>,
) -> Laid {
    let gutter = file_gutter(dv, gutter);
    let active = gutter.is_some_and(FilePreviewGutter::has_changes);
    let render_width = if active {
        width.saturating_sub(1)
    } else {
        width
    };
    let code = display.code_style();
    let details_open = HashMap::new();
    let settings = RenderSettings {
        width: render_width,
        code,
        code_theme: &display.theme.code_theme,
        icons: display.icons,
        mermaid_caption: mermaid_caption(lang),
        details_open: &details_open,
    };
    // A Markdown file cut at the display limit says so at its end (a converted document says so
    // in the frame title instead).
    let cut_note = dv.truncated && !matches!(dv.origin, Origin::Converted { .. });
    let cut_md;
    let md: &str = if cut_note {
        cut_md = format!("{}{TRUNCATED_NOTE}", dv.markdown);
        &cut_md
    } else {
        &dv.markdown
    };
    let mut r = match pics {
        Some(p) => p.render(dv, md, render_width, rows, lang, &settings),
        None => {
            let image = |_: &str, _: Option<u16>| ImageSlot::Unavailable;
            let mermaid = |_: &str| MermaidSlot::Text;
            let math = |_: &str, _: bool| MathSlot::Raw;
            render_document(
                md,
                &dv.options,
                &settings,
                &PictureSlots {
                    image: &image,
                    mermaid: &mermaid,
                    math: &math,
                    mermaid_on: false,
                    math_on: false,
                    remote_on: false,
                },
            )
        }
    };
    let (lines, _targets) = postprocess(r.lines, &mut r.images, &dv.options, true);
    let row_prefix = if display.wrap && render_width > 0 {
        wrap_row_prefix(&lines, render_width)
    } else {
        Vec::new()
    };
    let max_line_cols = lines.iter().map(|l| l.width()).max().unwrap_or(0) + usize::from(active);
    let marks = gutter.map_or_else(Vec::new, |g| g.project(&r.extras.block_rows));
    let lines = with_preview_gutter(lines, 0, &marks);
    if active {
        for image in &mut r.images {
            image.col += 1;
        }
    }
    Laid {
        lines,
        row_prefix,
        images: std::mem::take(&mut r.images),
        max_line_cols,
    }
}

fn file_gutter<'a>(
    dv: &DocumentView,
    gutter: Option<FilePreviewGutter<'a>>,
) -> Option<FilePreviewGutter<'a>> {
    match &dv.origin {
        Origin::File(path)
            if !path.extension().is_some_and(|e| {
                e.eq_ignore_ascii_case("mmd") || e.eq_ignore_ascii_case("mermaid")
            }) =>
        {
            gutter
        }
        _ => None,
    }
}

/// Draw through the document cache and the same visible-line slicing as the file app.
#[cfg(any(test, feature = "test-hooks"))]
pub fn draw_preview_document_for_test(
    dv: &DocumentView,
    outcome: Option<&crate::host::project::md_diff::MdDiffOutcome>,
    display: &Display,
    scroll: usize,
    buf: &mut Buffer,
    area: Rect,
    name: &str,
) {
    let inner = DocFrame::inner(area);
    let laid = DocCache::get_with_gutter(
        &mut None,
        dv,
        inner.width,
        inner.height,
        Lang::En,
        None,
        display,
        FilePreviewGutter::from_outcome(outcome),
    );
    let frame = DocFrame {
        name: name.to_string(),
        laid,
        scroll,
        wrap: display.wrap,
        hscroll: 0,
        lang: Lang::En,
        truncated: false,
    };
    frame.draw(buf, area);
}

/// Draw a prepared text/code window, including its line-number spans when enabled.
#[cfg(any(test, feature = "test-hooks"))]
pub fn draw_preview_lines_for_test(
    lines: Vec<RLine<'static>>,
    first_line: usize,
    marks: &HashMap<u32, GutterMark>,
    display: &Display,
    buf: &mut Buffer,
    area: Rect,
    file: (&str, &[u64]),
) {
    super::draw::TextFrame {
        lines: lay_out_lines(lines, first_line, Some(marks)),
        wrap: display.wrap,
        hscroll: 0,
        vscroll: 0,
        border: Some(super::draw::TextBorder {
            name: file.0.to_string(),
            lang: Lang::En,
            extent: Some(super::window::extent_of_lines(
                file.1,
                first_line,
                usize::from(area.height.saturating_sub(2)),
            )),
        }),
    }
    .draw(buf, area);
}

/// What the loop keeps so a key does not lay the page out again: the page is rebuilt only when
/// the text, the options, the width or the language change.
pub struct DocCache {
    markdown: String,
    options: DocOptions,
    truncated: bool,
    width: u16,
    rows: u16,
    lang: Lang,
    display: Display,
    /// The picture epoch it was laid out at: a result that changes what the page reserves makes
    /// it stale.
    epoch: u64,
    laid: Arc<Laid>,
    gutter_ops: Option<Vec<konoma_widgets::markdown::BlockOp>>,
}

impl DocCache {
    /// The layout of `dv` at `width`, from the cache when nothing changed.
    pub fn get(
        slot: &mut Option<DocCache>,
        dv: &DocumentView,
        width: u16,
        rows: u16,
        lang: Lang,
        pics: Option<&mut DocPics>,
        display: &Display,
    ) -> Arc<Laid> {
        Self::get_with_gutter(slot, dv, width, rows, lang, pics, display, None)
    }

    #[allow(clippy::too_many_arguments)]
    pub(in crate::app) fn get_with_gutter(
        slot: &mut Option<DocCache>,
        dv: &DocumentView,
        width: u16,
        rows: u16,
        lang: Lang,
        pics: Option<&mut DocPics>,
        display: &Display,
        gutter: Option<FilePreviewGutter<'_>>,
    ) -> Arc<Laid> {
        let gutter = file_gutter(dv, gutter);
        let gutter_ops = gutter.map(FilePreviewGutter::ops);
        let epoch = pics.as_deref().map_or(0, |p| p.epoch());
        if let Some(c) = slot.as_ref() {
            if c.width == width
                && (c.rows == rows || pics.is_none())
                && c.epoch == epoch
                && c.lang == lang
                && c.display == *display
                && c.options == dv.options
                && c.truncated == dv.truncated
                && c.markdown == dv.markdown
                && c.gutter_ops.as_deref() == gutter_ops
            {
                return Arc::clone(&c.laid);
            }
        }
        let laid = Arc::new(match gutter {
            Some(g) => lay_out_with_gutter(dv, width, rows, lang, pics, display, Some(g)),
            None => lay_out(dv, width, rows, lang, pics, display),
        });
        *slot = Some(DocCache {
            markdown: dv.markdown.clone(),
            options: dv.options.clone(),
            truncated: dv.truncated,
            width,
            rows,
            lang,
            display: display.clone(),
            epoch,
            laid: Arc::clone(&laid),
            gutter_ops: gutter_ops.map(<[_]>::to_vec),
        });
        laid
    }
}

/// The frame title: the path between spaces (the host's shape).
pub(super) fn path_title(name: &str) -> String {
    if name.is_empty() {
        " preview ".to_string()
    } else {
        format!(" {name} ")
    }
}

/// A document's frame title: the path, and the note of a converted document that was cut.
pub(super) fn doc_title(name: &str, truncated: bool, lang: Lang) -> String {
    let mut t = path_title(name);
    if truncated {
        t.push_str(CoreMsg::DocTruncatedTitle.tr(lang));
    }
    t
}

/// The words of the position label, in the reader's language.
pub(super) fn scroll_words(lang: Lang) -> ScrollWords {
    ScrollWords {
        all: CoreMsg::ScrollAll.tr(lang),
        top: CoreMsg::ScrollTop.tr(lang),
        bot: CoreMsg::ScrollBot.tr(lang),
    }
}

/// The furthest a document scrolls, in display rows: the host keeps the vertical scroll in a
/// `u16` (`preview_scroll`), and the row offset into the first line shown must fit the one a
/// `Paragraph` takes, so a line wrapped into 65536 rows or more never wraps around.
pub const MAX_DOC_SCROLL: usize = u16::MAX as usize;

/// A document on screen.
pub struct DocFrame {
    /// The path of the file as the host titles it (or the file name).
    pub name: String,
    pub laid: Arc<Laid>,
    /// The first display row shown.
    pub scroll: usize,
    /// The page wraps (`ui.wrap`). Off, it is drawn `hscroll` cells to the left.
    pub wrap: bool,
    /// Cells scrolled off the left edge (always 0 while wrapping).
    pub hscroll: u16,
    pub lang: Lang,
    /// A converted document cut at a budget (or a slide that left something out): the title says so.
    pub truncated: bool,
}

impl DocFrame {
    /// The frame title: the path between spaces (the host's shape), and the note of a converted
    /// document that was cut.
    fn title(&self) -> String {
        doc_title(&self.name, self.truncated, self.lang)
    }

    /// Where the page is: display rows, the end of the range being the last full page.
    fn extent(&self, height: u16) -> ScrollExtent {
        let h = u64::from(height);
        let max = (self.laid.rows() as u64).saturating_sub(h);
        ScrollExtent::new(self.scroll as u64, max, h)
    }

    /// The logical lines spanning the display rows `[scroll, scroll + height)` and the rows of
    /// the first one that are scrolled off (the host's `md_slice`).
    fn slice(&self, height: u16) -> (Vec<RLine<'static>>, u16) {
        let s = self.scroll;
        let (lines, pre) = (&self.laid.lines, &self.laid.row_prefix);
        let (range, local) = visible_line_range(lines.len(), pre, s, height);
        (
            lines[range].to_vec(),
            u16::try_from(local).unwrap_or(u16::MAX),
        )
    }

    /// The cells inside the frame's border.
    pub fn inner(body: Rect) -> Rect {
        Block::bordered().inner(body)
    }

    pub fn draw(&self, buf: &mut Buffer, body: Rect) {
        let inner = Self::inner(body);
        let extent = self.extent(inner.height);
        let words = scroll_words(self.lang);
        let block = Block::bordered()
            .title(self.title())
            .title(scroll_title(extent, words));
        let (lines, local) = self.slice(inner.height);
        let mut para = Paragraph::new(Text::from(lines)).block(block);
        if self.wrap {
            para = para.wrap(Wrap { trim: false });
        }
        para.scroll((local, self.hscroll)).render(body, buf);
        render_scrollbar(buf, body, extent);
    }
}

#[cfg(test)]
mod title_tests {
    use super::*;

    #[test]
    fn the_frame_title_is_the_path_or_preview_and_says_when_a_document_was_cut() {
        assert_eq!(path_title("sub/a.md"), " sub/a.md ");
        assert_eq!(path_title(""), " preview ");
        assert_eq!(doc_title("a.md", false, Lang::En), " a.md ");
        let cut = doc_title("a.md", true, Lang::En);
        assert_eq!(cut, " a.md · truncated (too large to show in full) ");
        assert!(doc_title("", true, Lang::Jp).starts_with(" preview · 打ち切り"));
    }
}

#[cfg(test)]
mod slice_tests {
    use super::*;

    fn frame(rows_of_the_one_line: usize, scroll: usize) -> DocFrame {
        DocFrame {
            name: String::new(),
            laid: Arc::new(Laid {
                lines: vec![RLine::from("x")],
                row_prefix: vec![0, rows_of_the_one_line],
                images: Vec::new(),
                max_line_cols: 1,
            }),
            scroll,
            wrap: true,
            hscroll: 0,
            lang: Lang::En,
            truncated: false,
        }
    }

    #[test]
    fn the_offset_into_a_line_wrapped_past_65535_rows_never_wraps_around() {
        // The host's furthest scroll: the offset is exactly the scroll.
        let (lines, local) = frame(100_000, MAX_DOC_SCROLL).slice(10);
        assert_eq!((lines.len(), local), (1, u16::MAX));
        // Past it (never reached: the loop caps the scroll), the offset saturates; `as u16`
        // gave 4464 here, a different part of the line.
        let (_, local) = frame(100_000, 70_000).slice(10);
        assert_eq!(local, u16::MAX);
    }
}
