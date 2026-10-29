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

use konoma_host_core::lang::Lang;
use konoma_widgets::markdown::{ImagePlacement, ImageSlot, MathSlot, MermaidSlot};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::{Line as RLine, Text};
use ratatui::widgets::{Block, Paragraph, Widget, Wrap};

use super::display::Display;
use super::doc_pictures::DocPics;
use crate::document_render::{
    postprocess, render_document, wrap_row_prefix, PictureSlots, RenderSettings,
};
use crate::view::{DocOptions, DocumentView};

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
    let code = display.code_style();
    let details_open = HashMap::new();
    let settings = RenderSettings {
        width,
        code,
        code_theme: &display.theme.code_theme,
        icons: display.icons,
        mermaid_caption: mermaid_caption(lang),
        details_open: &details_open,
    };
    let mut r = match pics {
        Some(p) => p.render(dv, width, rows, lang, &settings),
        None => {
            let image = |_: &str, _: Option<u16>| ImageSlot::Unavailable;
            let mermaid = |_: &str| MermaidSlot::Text;
            let math = |_: &str, _: bool| MathSlot::Raw;
            render_document(
                &dv.markdown,
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
    let row_prefix = if display.wrap && width > 0 {
        wrap_row_prefix(&lines, width)
    } else {
        Vec::new()
    };
    let max_line_cols = lines.iter().map(|l| l.width()).max().unwrap_or(0);
    Laid {
        lines,
        row_prefix,
        images: std::mem::take(&mut r.images),
        max_line_cols,
    }
}

/// What the loop keeps so a key does not lay the page out again: the page is rebuilt only when
/// the text, the options, the width or the language change.
pub struct DocCache {
    markdown: String,
    options: DocOptions,
    width: u16,
    rows: u16,
    lang: Lang,
    display: Display,
    /// The picture epoch it was laid out at: a result that changes what the page reserves makes
    /// it stale.
    epoch: u64,
    laid: Arc<Laid>,
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
        let epoch = pics.as_deref().map_or(0, |p| p.epoch());
        if let Some(c) = slot.as_ref() {
            if c.width == width
                && (c.rows == rows || pics.is_none())
                && c.epoch == epoch
                && c.lang == lang
                && c.display == *display
                && c.options == dv.options
                && c.markdown == dv.markdown
            {
                return Arc::clone(&c.laid);
            }
        }
        let laid = Arc::new(lay_out(dv, width, rows, lang, pics, display));
        *slot = Some(DocCache {
            markdown: dv.markdown.clone(),
            options: dv.options.clone(),
            width,
            rows,
            lang,
            display: display.clone(),
            epoch,
            laid: Arc::clone(&laid),
        });
        laid
    }
}

/// A document on screen.
pub struct DocFrame {
    /// The file name, in the frame title.
    pub name: String,
    pub laid: Arc<Laid>,
    /// The first display row shown.
    pub scroll: usize,
    /// The page wraps (`ui.wrap`). Off, it is drawn `hscroll` cells to the left.
    pub wrap: bool,
    /// Cells scrolled off the left edge (always 0 while wrapping).
    pub hscroll: u16,
}

impl DocFrame {
    /// The frame title: the file name between spaces (the host's shape), or none.
    fn title(&self) -> String {
        if self.name.is_empty() {
            " document ".to_string()
        } else {
            format!(" {} ", self.name)
        }
    }

    /// The logical lines spanning the display rows `[scroll, scroll + height)` and the rows of
    /// the first one that are scrolled off (the host's `md_slice`).
    fn slice(&self, height: u16) -> (Vec<RLine<'static>>, u16) {
        let s = self.scroll;
        let h = usize::from(height.max(1));
        let (lines, pre) = (&self.laid.lines, &self.laid.row_prefix);
        let (lo, hi, local) = if pre.len() == lines.len() + 1 {
            let lo = pre.partition_point(|&p| p <= s).saturating_sub(1);
            let hi = pre.partition_point(|&p| p < s + h).min(lines.len());
            (lo, hi.max(lo), (s - pre[lo]) as u16)
        } else {
            (s.min(lines.len()), (s + h).min(lines.len()), 0)
        };
        (lines[lo..hi].to_vec(), local)
    }

    /// The cells inside the frame's border.
    pub fn inner(body: Rect) -> Rect {
        Block::bordered().inner(body)
    }

    pub fn draw(&self, buf: &mut Buffer, body: Rect) {
        let block = Block::bordered().title(self.title());
        let inner = block.inner(body);
        let (lines, local) = self.slice(inner.height);
        let mut para = Paragraph::new(Text::from(lines)).block(block);
        if self.wrap {
            para = para.wrap(Wrap { trim: false });
        }
        para.scroll((local, self.hscroll)).render(body, buf);
    }
}
