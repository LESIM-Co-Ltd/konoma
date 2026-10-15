//! The display types a plugin returns. The host draws them with its own widgets.

use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crate::input::{LazyResource, Origin, Resources};
use crate::manifest::ViewKind;
use crate::style::{Line, Style};
use crate::text::Tr;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// How a session can be shown: its representations and its pages.
///
/// **`R` and the representations.** With two or more [`ReprDecl`]s the host gives the plugin `R`
/// for free: it asks for the next one in the declared order (wrapping after the last) and names
/// it with the plugin's label in the footer and the help. The ids are only names to the host.
/// There is **one exception**: a document ([`ViewKind::Document`]) that declares exactly the two
/// representations `"rendered"` and `"source"` (in either order), and a standalone diagram file,
/// get the host's own rendered / source toggle (the host's wording; the source is shown in its
/// windowed reader, so it can be searched and copied). A document that declares one
/// representation or none has no `R`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Layout {
    /// Named representations (for example a picture and a text form). Empty means just one.
    pub reprs: Vec<ReprDecl>,
    /// The representation shown first.
    pub default_repr: Option<String>,
    /// The first representation on a terminal that cannot draw pixels.
    pub default_repr_without_pixels: Option<String>,
    /// Pages, if the content has them.
    pub pages: Option<PageLayout>,
}

impl Layout {
    /// One representation, no pages.
    pub fn single() -> Layout {
        Layout::default()
    }
}

/// One named representation. The `id` is the name the plugin is asked for ([`ViewReq::repr`]);
/// any string will do, `"source"` included, except that a document declaring exactly `"rendered"`
/// and `"source"` gets the host's rendered / source toggle (see [`Layout`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReprDecl {
    pub id: String,
    pub label: Tr,
}

impl ReprDecl {
    pub fn new(id: impl Into<String>, label: Tr) -> ReprDecl {
        ReprDecl {
            id: id.into(),
            label,
        }
    }
}

/// How the content is split into pages.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PageLayout {
    /// Separate pages, fetched one at a time (PDF pages, spreadsheet sheets). `count` may be
    /// unknown until the first page is read.
    Separate {
        count: Option<usize>,
        titles: Vec<String>,
    },
    /// One long document whose pages start at headings of `heading_level` (slides).
    Anchored { heading_level: u8, count: usize },
}

/// What the host asks `Session::view` for.
///
/// Build one with [`ViewReq::new`] and the builders (more fields may be added later).
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct ViewReq {
    /// The representation (`None` = the default).
    pub repr: Option<String>,
    /// The page, counted from 0 (`None` = the first or the whole). A page past the end is clamped
    /// to the last one.
    pub page: Option<usize>,
    /// The size of the area in cells: what the host will draw the view into. It is the real size
    /// (the preview's content area), either may be 0, and it changes when the user resizes the
    /// terminal; the host then asks again (a canvas) with the new size. Before the first frame has
    /// been measured it is a placeholder (80 × 1); a canvas is asked again once the real size is
    /// known.
    pub cols: u16,
    pub rows: u16,
    /// The size of one cell in pixels as (width, height), if known. `None` when the terminal
    /// draws no real pixels (no graphics backend, or half blocks only); canvas pictures may still
    /// be drawn as half blocks then.
    pub cell_px: Option<(u16, u16)>,
}

impl ViewReq {
    pub fn new(cols: u16, rows: u16) -> ViewReq {
        ViewReq {
            repr: None,
            page: None,
            cols,
            rows,
            cell_px: None,
        }
    }

    /// The same asking for the representation `repr`.
    pub fn repr(mut self, repr: impl Into<String>) -> ViewReq {
        self.repr = Some(repr.into());
        self
    }

    /// The same asking for the page `page` (from 0).
    pub fn page(mut self, page: usize) -> ViewReq {
        self.page = Some(page);
        self
    }

    /// The same with the size of one cell in pixels.
    pub fn cell_px(mut self, w: u16, h: u16) -> ViewReq {
        self.cell_px = Some((w, h));
        self
    }
}

impl Default for ViewReq {
    fn default() -> ViewReq {
        ViewReq::new(80, 24)
    }
}

/// A display type. More may be added later: match with a `_` arm.
#[derive(Debug)]
#[non_exhaustive]
pub enum View {
    Text(TextView),
    Document(DocumentView),
    Table(TableView),
    Image(ImageView),
    List(ListView),
    Diff(DiffView),
    /// Cells and picture placements the plugin draws itself.
    Canvas(CanvasView),
    /// "Open this again as that content type" (a converted document, say).
    Reopen(Reopen),
    Message(MessageView),
}

impl View {
    /// The display type this view is (`None` for a [`View::Message`], a notice every plugin may
    /// show in place of its content). A [`View::Reopen`] is a document.
    pub fn kind(&self) -> Option<ViewKind> {
        match self {
            View::Text(_) => Some(ViewKind::Text),
            View::Document(_) | View::Reopen(_) => Some(ViewKind::Document),
            View::Table(_) => Some(ViewKind::Table),
            View::Image(_) => Some(ViewKind::Image),
            View::List(_) => Some(ViewKind::List),
            View::Diff(_) => Some(ViewKind::Diff),
            View::Canvas(_) => Some(ViewKind::Canvas),
            View::Message(_) => None,
        }
    }

    /// Whether a plugin that declared `shows` ([`crate::Manifest::shows`]) may return this view.
    /// A message always fits. A text view also fits a plugin that declares a document or a
    /// picture: the source of a document and the text form of a diagram or formula are texts by
    /// convention (their `R`).
    pub fn fits(&self, shows: &[ViewKind]) -> bool {
        match self.kind() {
            None => true,
            Some(ViewKind::Text) => shows
                .iter()
                .any(|k| matches!(k, ViewKind::Text | ViewKind::Document | ViewKind::Image)),
            Some(kind) => shows.contains(&kind),
        }
    }

    /// A text view of ready-made lines (strings, spans or [`Line`]s).
    pub fn text<I>(lines: I) -> View
    where
        I: IntoIterator,
        I::Item: Into<Line>,
    {
        View::Text(TextView {
            source: TextSource::Lines(lines.into_iter().map(Into::into).collect()),
            styler: None,
            file_lines: false,
            syntax_name: None,
        })
    }
}

/// Where the lines of a text view come from.
#[derive(Debug)]
pub enum TextSource {
    /// A file the host reads in windows (large files stay cheap).
    File(PathBuf),
    /// A string.
    Owned(String),
    /// Lines already styled.
    Lines(Vec<Line>),
}

/// Colors the visible lines of a text view. The host calls it synchronously for the visible block
/// on every frame, so it must be light: do the heavy preparation in `open`.
pub trait LineStyler: Send {
    /// Style `text` (whole lines, newline separated) that starts at the 0-based `first_line`.
    fn style_block(&mut self, first_line: usize, text: &str) -> Vec<Line>;
    /// Whether the styler is ready. While false the host shows plain text and a note.
    fn ready(&self) -> bool {
        true
    }
}

/// A text view.
pub struct TextView {
    pub source: TextSource,
    pub styler: Option<Box<dyn LineStyler>>,
    /// Whether line numbers equal the numbers in the original file (used by `e`, `Y` and paste
    /// jumps). False for converted content.
    pub file_lines: bool,
    /// A syntax name for the host's highlighter when there is no styler.
    pub syntax_name: Option<String>,
}

impl fmt::Debug for TextView {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TextView")
            .field("source", &self.source)
            .field("styler", &self.styler.as_ref().map(|_| "<styler>"))
            .field("file_lines", &self.file_lines)
            .field("syntax_name", &self.syntax_name)
            .finish()
    }
}

/// A document: Markdown (CommonMark and GFM with konoma's extensions) plus resources.
#[derive(Debug)]
pub struct DocumentView {
    pub markdown: String,
    pub origin: Origin,
    pub resources: Resources,
    pub options: DocOptions,
    pub title: Option<String>,
    /// Whether the content was cut at the display limit.
    pub truncated: bool,
}

/// How a document is rendered. Names and values follow the `md_*` settings; the defaults are
/// the defaults of those settings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocOptions {
    pub front_matter: bool,
    pub footnotes: bool,
    pub inline_html: bool,
    pub autolink: bool,
    pub emoji: bool,
    pub alerts: bool,
    /// `"auto"`, `"open"` or `"closed"`-style value of `md_details`, as is.
    pub details: String,
    /// The states a task checkbox cycles through.
    pub task_states: Vec<char>,
    /// Whether checkboxes may be written back to the file.
    pub tasks_writable: bool,
    /// The `md_table_align` value, as is.
    pub table_align: String,
    /// The `md_image_align` value, as is.
    pub image_align: String,
}

impl DocOptions {
    /// The options a `[plugins.markdown]` table (with the old `[ui] md_*` values filled in by the
    /// host) asks for. A key that is missing or of another type keeps its default. `task_states`
    /// must be at least two single-character entries, or the default pair is used (a cycle needs
    /// somewhere to go). `tasks_writable` stays true: whether a document can be written back is
    /// decided by where it came from, not by a setting.
    pub fn from_settings(settings: &crate::settings::Settings) -> DocOptions {
        let d = DocOptions::default();
        let raw = settings.list_str("task_states");
        let chars: Vec<char> = raw
            .iter()
            .filter_map(|s| {
                let mut it = s.chars();
                let c = it.next()?;
                it.next().is_none().then_some(c)
            })
            .collect();
        let task_states = if chars.len() >= 2 && chars.len() == raw.len() {
            chars
        } else {
            d.task_states.clone()
        };
        DocOptions {
            front_matter: settings.bool("frontmatter", d.front_matter),
            footnotes: settings.bool("footnotes", d.footnotes),
            inline_html: settings.bool("inline_html", d.inline_html),
            autolink: settings.bool("autolink", d.autolink),
            emoji: settings.bool("emoji", d.emoji),
            alerts: settings.bool("alerts", d.alerts),
            details: settings.str("details", &d.details).to_string(),
            task_states,
            tasks_writable: d.tasks_writable,
            table_align: settings.str("table_align", &d.table_align).to_string(),
            image_align: settings.str("image_align", &d.image_align).to_string(),
        }
    }
}

impl Default for DocOptions {
    fn default() -> DocOptions {
        DocOptions {
            front_matter: true,
            footnotes: true,
            inline_html: true,
            autolink: true,
            emoji: true,
            alerts: true,
            details: "auto".into(),
            task_states: vec![' ', 'x'],
            tasks_writable: true,
            table_align: "left".into(),
            image_align: "center".into(),
        }
    }
}

/// A table, read cell by cell so a big table stays cheap.
pub trait Grid: Send + Sync {
    fn nrows(&self) -> usize;
    fn ncols(&self) -> usize;
    fn header(&self, col: usize) -> Option<String>;
    fn cell(&self, row: usize, col: usize) -> String;
    /// The cell as a borrowed string, for a grid that keeps its cells as text. The host reads
    /// every cell when it searches, and this lets it do so without copying each one. Return `None`
    /// (the default) if the text is made on demand; the host then calls [`Grid::cell`].
    fn cell_str(&self, row: usize, col: usize) -> Option<&str> {
        let _ = (row, col);
        None
    }
    /// How many cells `row` really has, for a ragged table whose short rows are padded when drawn.
    /// The host copies a row with this many cells and no padding. The default is every column.
    fn row_len(&self, row: usize) -> usize {
        let _ = row;
        self.ncols()
    }
    /// Whether the table was cut at a limit.
    fn truncated(&self) -> bool {
        false
    }
    /// Calls `f(col, text)` for each cell of `row` that has something to search, in column order.
    /// The host searches a table with this. The default visits every column; a sparse grid (a
    /// sheet 16,000 columns wide with a few cells per row) overrides it to visit only the cells
    /// that exist, so a search costs what the sheet holds and not what it spans.
    fn visit_row(&self, row: usize, f: &mut dyn FnMut(usize, &str)) {
        for col in 0..self.ncols() {
            match self.cell_str(row, col) {
                Some(text) => f(col, text),
                None => f(col, &self.cell(row, col)),
            }
        }
    }
    /// The detail lines for the cell popup (type, shown value, raw value, format).
    fn detail(&self, row: usize, col: usize) -> Option<CellDetail> {
        let _ = (row, col);
        None
    }
}

/// Labeled lines for the full-cell popup.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CellDetail {
    pub lines: Vec<(Tr, String)>,
}

/// A table view.
pub struct TableView {
    pub grid: Arc<dyn Grid>,
    /// A note the host puts after the file name in the table's frame title (a spreadsheet's sheet
    /// name and position, for example). `None` keeps the host's own title: the file name and the
    /// cursor position.
    pub title: Option<String>,
    /// Color the columns in turn.
    pub rainbow: bool,
    /// Whether the cells have spreadsheet addresses. The host then draws a row-number gutter, puts
    /// the cursor cell's address (`B3`) in the title, names the full-cell popup by it, and copies
    /// only what is shown: a row is the shown cells joined by tabs, a column has no header line.
    /// The grid's [`Grid::header`] is the column letters.
    pub addressed: bool,
}

impl fmt::Debug for TableView {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TableView")
            .field("rows", &self.grid.nrows())
            .field("cols", &self.grid.ncols())
            .field("title", &self.title)
            .field("rainbow", &self.rainbow)
            .field("addressed", &self.addressed)
            .finish()
    }
}

/// An image view.
#[derive(Clone, Debug)]
pub struct ImageView {
    pub data: ImageData,
    /// Redraw sharply at the zoomed size (vector sources).
    pub sharp_zoom: bool,
    /// The longest side, in pixels, the host should rasterize a vector source at (`None` = the
    /// host's own default). The SVG plugin carries its `max_px` setting here.
    pub raster_max_px: Option<u32>,
}

impl ImageView {
    /// An image with no sharp zoom and the host's default raster size.
    pub fn new(data: ImageData) -> ImageView {
        ImageView {
            data,
            sharp_zoom: false,
            raster_max_px: None,
        }
    }
}

/// The pixels (or the way to get them) of an image.
#[derive(Clone, Debug)]
pub enum ImageData {
    File(PathBuf),
    /// An encoded image (PNG, JPEG...).
    Encoded(Arc<[u8]>),
    /// SVG. `trusted: false` makes the host draw it in a child process with limits.
    ///
    /// **`trusted` is a claim, not a decision.** The host believes it only for a standard plugin
    /// that draws the SVG itself (the mermaid and math plugins: the SVG is made by konoma's own
    /// code from a diagram or a formula). For any other plugin, whatever it claims, and for any
    /// SVG that comes from a file or a document, the host draws it in the supervised child
    /// process, and reads no file outside `base_dir`'s file name. Set `trusted: true` only when
    /// the plugin made the SVG from scratch; the value is an upper bound, never a way to skip
    /// the limits.
    Svg {
        data: Arc<[u8]>,
        base_dir: Option<PathBuf>,
        trusted: bool,
    },
    /// Raw RGBA pixels, row by row. Held as an `Arc<Vec<u8>>` so a host that is the only owner can
    /// take the buffer without copying it (`Arc::try_unwrap`).
    Rgba {
        width: u32,
        height: u32,
        pixels: Arc<Vec<u8>>,
    },
    /// Animation frames with their durations.
    Frames(Vec<(ImageData, Duration)>),
    /// Made when visible.
    Lazy(Arc<dyn LazyResource>),
}

/// A list of rows (mostly project screens).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ListView {
    pub rows: Vec<ListRow>,
}

/// One row of a list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListRow {
    pub cells: Vec<Line>,
    /// The kind of row (decides which actions apply).
    pub kind: String,
    /// The place the row points at, for `e`, `o` and a new tab.
    pub target: Option<PathBuf>,
    /// A check state, for rows that can be selected as staged and so on.
    pub checked: Option<bool>,
}

/// A diff.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DiffView {
    pub hunks: Vec<DiffHunk>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DiffHunk {
    pub lines: Vec<DiffLine>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: DiffKind,
    pub old_no: Option<usize>,
    pub new_no: Option<usize>,
    pub text: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffKind {
    Context,
    Added,
    Removed,
}

/// "Open this again as `content_type`."
#[derive(Debug)]
pub struct Reopen {
    pub content_type: String,
    pub text: String,
    pub resources: Resources,
    pub origin: Origin,
    /// Whether the conversion was cut at a limit (the end, some pictures, or part of a table is
    /// missing); the host says so in the document's title.
    pub truncated: bool,
}

/// A short message shown in place of the content.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MessageView {
    pub lines: Vec<Line>,
}

/// One cell of a canvas: a symbol (one grapheme) and its style.
///
/// The symbol of a wide character (CJK, most emoji) sits in its left cell and the cell to its
/// right holds the empty string (a *continuation*), the same as ratatui's buffer. The default cell
/// is one space with the default style.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CanvasCell {
    pub symbol: String,
    pub style: Style,
}

impl CanvasCell {
    /// A cell of `symbol` in `style`.
    pub fn new(symbol: impl Into<String>, style: Style) -> CanvasCell {
        CanvasCell {
            symbol: symbol.into(),
            style,
        }
    }

    /// Whether `symbol` may sit in a cell as it is: one grapheme of at most
    /// [`CANVAS_MAX_SYMBOL_BYTES`] bytes with no control character, 1 or 2 columns wide. (The
    /// empty continuation is not a symbol; [`CanvasView::check`] accepts it after a wide
    /// character.) Code that fills cells from another source can use this to blank what fails.
    pub fn is_plain_symbol(symbol: &str) -> bool {
        !symbol.is_empty() && symbol_fault(symbol).is_none()
    }

    /// Whether this is the empty right half of a wide character.
    pub fn is_continuation(&self) -> bool {
        self.symbol.is_empty()
    }
}

impl Default for CanvasCell {
    fn default() -> CanvasCell {
        CanvasCell {
            symbol: " ".to_string(),
            style: Style::default(),
        }
    }
}

/// A rectangle of canvas cells.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CellRect {
    pub x: u16,
    pub y: u16,
    pub cols: u16,
    pub rows: u16,
}

impl CellRect {
    pub fn new(x: u16, y: u16, cols: u16, rows: u16) -> CellRect {
        CellRect { x, y, cols, rows }
    }
}

/// The most cells a canvas may be wide or tall.
pub const CANVAS_MAX_SIDE: u16 = 4096;
/// The most cells a canvas may hold in all (`cols * rows`).
pub const CANVAS_MAX_CELLS: usize = 1 << 22;
/// The most picture placements a canvas may hold.
pub const CANVAS_MAX_PLACEMENTS: usize = 4096;
/// The most distinct picture ids a canvas may use.
pub const CANVAS_MAX_PICTURES: usize = 256;
/// The longest symbol of a cell, in bytes (a base character with its marks, or a joined emoji).
pub const CANVAS_MAX_SYMBOL_BYTES: usize = 32;
/// The longest side, in pixels, of a raw [`ImageData::Rgba`] picture on a canvas. This is the
/// side the host keeps decoded pictures at.
pub const CANVAS_MAX_RGBA_SIDE: u32 = 4096;
/// The most frames an animation ([`ImageData::Frames`]) on a canvas may have.
pub const CANVAS_MAX_FRAMES: usize = 5000;

/// A picture placed on a canvas, covering `cols` x `rows` cells from (`x`, `y`).
///
/// **`id` is the picture's identity**, chosen by the plugin and scoped to the opened file. The host
/// decodes the pixels once, keeps the terminal's stored copy under the same `id`, and does not
/// redo the work when the plugin answers with the same `id` and the same content again (an
/// animation that moves a sprite sends the sprite once):
///
/// - the same `id` with the same content: the same picture, nothing is redone;
/// - the same `id` with changed content: the picture is **replaced** in place (the old one keeps
///   drawing until the new one is ready, so it does not blink). Reusing one `id` for a "slot"
///   (the album art, the current frame) is fine and is the way to avoid leaking terminal memory;
/// - a new `id`: a new picture. An `id` the canvas stops naming is dropped;
/// - the same `id` placed twice in one answer: the same picture at two places (a tiled sprite).
///
/// The content is compared on every answer (every byte of `Rgba` and `Encoded` data, the path,
/// size and mtime of a `File`), so keep one `Arc` and clone it into each answer rather than
/// rebuilding a large buffer per frame. [`ImageData::Lazy`] is not allowed on a canvas, which is
/// plain data ([`CanvasView::check`] refuses it).
///
/// **`image: None` means "unchanged"**: the picture the last answer named for this `id`, so a
/// redraw that only moves a sprite need not send its pixels again. A `None` for an `id` the host
/// does not hold (the first answer, or one the canvas stopped naming) shows the cells instead.
/// You never have to send a picture again because of a limit: the host keeps what it has not
/// decoded yet (left for a later answer by the per-answer limits, or refused for now by the
/// memory bound) and decodes it on a later answer even when that answer says `None`. Only newer
/// `Some` content for the `id` replaces what the host kept; it is released when the canvas stops
/// naming the `id`.
///
/// **How a picture is fitted.** It is scaled to fit inside the `cols` x `rows` cells with its
/// aspect ratio kept, starting at the top-left of the rectangle, and it is not enlarged past its
/// own pixel size. For a pixel-exact result make an `Rgba` picture `cols * cell_px.0` by
/// `rows * cell_px.1` pixels ([`ViewReq::cell_px`], which is (width, height)).
///
/// **Limits** (the host's, they may change; the `CANVAS_MAX_*` constants are checked by
/// [`CanvasView::check`], which refuses the whole canvas):
///
/// - at most [`CANVAS_MAX_PLACEMENTS`] placements and [`CANVAS_MAX_PICTURES`] distinct ids per
///   canvas; a raw `Rgba` side of at most [`CANVAS_MAX_RGBA_SIDE`] pixels; an animation of at most
///   [`CANVAS_MAX_FRAMES`] frames, not nested;
/// - the host holds at most 256 MiB of decoded canvas pixels over all files. A picture that would
///   not fit is refused for now, not failed: it is decoded on a later answer once room is free
///   (the pictures the canvas no longer names go first);
/// - per answer it decodes at most 32 new pictures, 64 MiB of encoded input and 128 MiB of new
///   pixels (their decoded size); the rest follow on later answers. The cells under a picture that
///   is not drawn (yet) stay, so draw something sensible there;
/// - a frame of an animation is shown for at least 20 ms;
/// - a picture from a file must be a regular file (not a pipe or a device);
/// - a picture that cannot be decoded (damaged, too large, an SVG that is refused) is not retried
///   for the same content;
/// - a picture that starts outside the area is not drawn; one that reaches past the right edge is
///   not drawn at all; one that reaches below the bottom is cut to the visible rows. Keep
///   pictures inside `ViewReq::cols` x `rows`;
/// - on a terminal with no graphics nothing is decoded and the cells stay; with half blocks only,
///   pictures are drawn as half blocks. [`crate::TermCaps::pixels`] tells which.
///
/// Build one with [`CanvasImage::new`] (or [`CanvasImage::unchanged`]); more fields may be added
/// later.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct CanvasImage {
    pub id: u64,
    pub x: u16,
    pub y: u16,
    pub cols: u16,
    pub rows: u16,
    /// The pixels, or `None` for the picture the host already holds under `id`.
    pub image: Option<ImageData>,
}

impl CanvasImage {
    /// The picture `image` named `id`, covering `rect`.
    pub fn new(id: u64, rect: CellRect, image: ImageData) -> CanvasImage {
        CanvasImage {
            id,
            x: rect.x,
            y: rect.y,
            cols: rect.cols,
            rows: rect.rows,
            image: Some(image),
        }
    }

    /// The picture the host already holds under `id` (sent in an earlier answer), covering `rect`.
    pub fn unchanged(id: u64, rect: CellRect) -> CanvasImage {
        CanvasImage {
            id,
            x: rect.x,
            y: rect.y,
            cols: rect.cols,
            rows: rect.rows,
            image: None,
        }
    }

    /// The cells this picture covers.
    pub fn rect(&self) -> CellRect {
        CellRect::new(self.x, self.y, self.cols, self.rows)
    }
}

/// What is wrong with a canvas ([`CanvasView::check`]). The host refuses such a canvas with a
/// message instead of drawing it. More may be added later: match with a `_` arm.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum CanvasIssue {
    /// `cells.len()` is not `cols * rows`.
    CellCount { expected: usize, actual: usize },
    /// `cols` or `rows` is over [`CANVAS_MAX_SIDE`], or `cols * rows` over [`CANVAS_MAX_CELLS`].
    AreaTooLarge { cols: u16, rows: u16 },
    /// The symbol of the cell at (`x`, `y`) holds a control character (C0, DEL or C1).
    CellControl { x: u16, y: u16 },
    /// The symbol of the cell at (`x`, `y`) is longer than [`CANVAS_MAX_SYMBOL_BYTES`].
    CellSymbolTooLong { x: u16, y: u16 },
    /// The symbol of the cell at (`x`, `y`) is more than one grapheme.
    CellNotOneGrapheme { x: u16, y: u16 },
    /// The symbol of the cell at (`x`, `y`) is wider than 2 columns, or has no width at all.
    CellBadWidth { x: u16, y: u16 },
    /// The wide character in the cell at (`x`, `y`) is not followed by an empty continuation cell
    /// (it is the last column, or the next cell has a symbol).
    WideWithoutContinuation { x: u16, y: u16 },
    /// The empty cell at (`x`, `y`) does not follow a wide character.
    StrayContinuation { x: u16, y: u16 },
    /// More than [`CANVAS_MAX_PLACEMENTS`] picture placements.
    TooManyPlacements { count: usize },
    /// More than [`CANVAS_MAX_PICTURES`] distinct picture ids.
    TooManyPictures { count: usize },
    /// Image number `index` reaches outside the canvas.
    ImageOutOfBounds { index: usize },
    /// Image number `index` covers no cell.
    ImageZeroSize { index: usize },
    /// Image number `index` is [`ImageData::Lazy`] (also inside animation frames).
    ImageLazy { index: usize },
    /// Image number `index` is raw pixels whose byte count is not `width * height * 4`, or
    /// that has a zero side (also inside animation frames).
    ImageRgbaSize { index: usize },
    /// Image number `index` is raw pixels with a side over [`CANVAS_MAX_RGBA_SIDE`].
    ImageTooLarge { index: usize },
    /// Image number `index` is an animation inside an animation.
    ImageNestedFrames { index: usize },
    /// Image number `index` is an animation of more than [`CANVAS_MAX_FRAMES`] frames.
    ImageTooManyFrames { index: usize },
}

impl fmt::Display for CanvasIssue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CanvasIssue::CellCount { expected, actual } => {
                write!(f, "canvas has {actual} cells, expected {expected}")
            }
            CanvasIssue::AreaTooLarge { cols, rows } => {
                write!(f, "canvas of {cols}x{rows} cells is too large")
            }
            CanvasIssue::CellControl { x, y } => {
                write!(f, "canvas cell ({x}, {y}) holds a control character")
            }
            CanvasIssue::CellSymbolTooLong { x, y } => {
                write!(f, "canvas cell ({x}, {y}) has too long a symbol")
            }
            CanvasIssue::CellNotOneGrapheme { x, y } => {
                write!(f, "canvas cell ({x}, {y}) is not exactly one character")
            }
            CanvasIssue::CellBadWidth { x, y } => {
                write!(f, "canvas cell ({x}, {y}) is not 1 or 2 columns wide")
            }
            CanvasIssue::WideWithoutContinuation { x, y } => {
                write!(
                    f,
                    "canvas cell ({x}, {y}) is a wide character with no empty cell after it"
                )
            }
            CanvasIssue::StrayContinuation { x, y } => {
                write!(
                    f,
                    "canvas cell ({x}, {y}) is empty but follows no wide character"
                )
            }
            CanvasIssue::TooManyPlacements { count } => {
                write!(f, "canvas has {count} pictures placed, too many")
            }
            CanvasIssue::TooManyPictures { count } => {
                write!(f, "canvas uses {count} different pictures, too many")
            }
            CanvasIssue::ImageOutOfBounds { index } => {
                write!(f, "canvas image {index} is outside the canvas")
            }
            CanvasIssue::ImageZeroSize { index } => {
                write!(f, "canvas image {index} covers no cell")
            }
            CanvasIssue::ImageLazy { index } => {
                write!(
                    f,
                    "canvas image {index} is lazy, which a canvas cannot hold"
                )
            }
            CanvasIssue::ImageRgbaSize { index } => {
                write!(f, "canvas image {index} has a wrong pixel byte count")
            }
            CanvasIssue::ImageTooLarge { index } => {
                write!(f, "canvas image {index} has too many pixels")
            }
            CanvasIssue::ImageNestedFrames { index } => {
                write!(
                    f,
                    "canvas image {index} has an animation inside an animation"
                )
            }
            CanvasIssue::ImageTooManyFrames { index } => {
                write!(f, "canvas image {index} has too many frames")
            }
        }
    }
}

impl std::error::Error for CanvasIssue {}

/// A view the plugin draws itself: a grid of styled cells plus picture placements. It is plain
/// data (no closures), so it can cross a process boundary as it is.
///
/// Colors: the host does not convert. Read [`crate::TermCaps::colors`] and pick colors the
/// terminal can show.
///
/// **What [`CanvasView::check`] guarantees** (the host refuses a canvas that fails it, and the
/// cell-writing methods here only produce cells that pass): every cell symbol is a single
/// grapheme of at most [`CANVAS_MAX_SYMBOL_BYTES`] bytes with no control character (C0, DEL, C1)
/// and 1 or 2 columns wide, or empty (the continuation of the wide character to its left); the
/// area is at most [`CANVAS_MAX_SIDE`] per side and [`CANVAS_MAX_CELLS`] cells; and the pictures
/// are within the `CANVAS_MAX_*` limits. Build one with [`CanvasView::new`]; more fields may be
/// added later.
///
/// **The size is the plugin's to fit, the host clips.** Size it to the request:
/// `CanvasView::new(req.cols, req.rows)`. A resize can race an answer, so a canvas bigger than the
/// area asked for (in either dimension, up to the limits above) is not refused: it is drawn
/// clipped at the right and bottom edges (a wide character cut by the edge is blanked). The host
/// asks again when the *area* changes (a resize), not because a canvas is bigger than it was asked
/// for, so a plugin that always returns a larger canvas is simply clipped. A smaller one leaves the rest of the area blank. The
/// canvas is drawn at the top-left of the preview's content area. `cols` and `rows` can be 0.
///
/// **`Session::view` is called at times the plugin does not choose**: on the first measure of the
/// area, on every resize, after a `Redraw` answer to a key or an action, and on each timer tick.
/// So it must not advance state (do not count calls to animate): keep it a function of the
/// session's state, and take time from an `Instant` stored at `open`.
///
/// A canvas has no scrolling, search or selection from the host (see
/// [`crate::keys::reserved`]), and no cursor: draw your own caret.
///
/// ```
/// use konoma_plugin::prelude::*;
///
/// struct Hello;
/// impl Session for Hello {
///     fn view(&mut self, req: &ViewReq, cx: &ViewCx) -> Result<View, Failure> {
///         let mut canvas = CanvasView::new(req.cols, req.rows);
///         // Colors are the plugin's choice: the host does not convert them.
///         let style = match cx.term.colors {
///             ColorDepth::TrueColor => Style::new().fg(Color::Rgb(255, 160, 40)),
///             _ => Style::new().fg(Color::Yellow),
///         };
///         canvas.set_string(1, 0, "hello, 世界", style);
///         Ok(View::Canvas(canvas))
///     }
/// }
///
/// let req = ViewReq::new(20, 2);
/// let View::Canvas(canvas) = Hello.view(&req, &ViewCx::bare()).unwrap() else {
///     panic!("a canvas")
/// };
/// assert_eq!((canvas.cols, canvas.rows), (20, 2));
/// assert!(canvas.check().is_ok());
/// ```
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct CanvasView {
    pub cols: u16,
    pub rows: u16,
    /// Row by row, `cols * rows` cells.
    pub cells: Vec<CanvasCell>,
    /// Drawn over the cells, in order.
    pub images: Vec<CanvasImage>,
    /// Ask the host to call `view` again after this long. It is **one-shot**: it applies to this
    /// answer only, so an animation sets it on every view it returns, and the timer stops when an
    /// answer leaves it `None`. It is a clamp, not a rule: values under 33 ms are raised to 33 ms
    /// (about 30 frames a second) and values over 24 hours are lowered to 24 hours. The time runs
    /// from the moment the answer lands, so the real period is the delay plus the time `view`
    /// takes. Only while the canvas is on screen and fully visible: under the help screen or a
    /// dialog the plugin is not asked, and it is asked once when that closes; in the tree, on
    /// another file or another tab it is not asked at all. A tick that falls while a call is still
    /// running is skipped, not queued, so measure time with `Instant` and do not count calls.
    pub redraw_after: Option<Duration>,
    /// Ask the host to send every key but `Esc` to [`crate::Session::key`].
    ///
    /// **`Esc` leaves, and only the user comes back.** `Esc` always leaves the text input, whatever
    /// the plugin answers. After it the host does not take the typing again because a later view
    /// asks for it (a timer redraw, a resize, the answer to a key): it does when the canvas that
    /// asks is the answer to something the user did -- a declared action they pressed, or opening
    /// the file again. So a plugin that wants a "typing mode" needs an action (a key such as `i`)
    /// that enters it, and its `Esc` handling should show a canvas with `text_input: false`.
    ///
    /// While it is on, **every key but `Esc` is the plugin's**, layer-0 keys (`?`, `Q`, `t`,
    /// `[`, `1`-`9`...) and `Ctrl-C` included: they are text. Show a hint that `Esc` leaves. The
    /// host puts "input" in the status line and the footer.
    pub text_input: bool,
}

impl CanvasView {
    /// A blank canvas.
    pub fn new(cols: u16, rows: u16) -> CanvasView {
        CanvasView {
            cols,
            rows,
            cells: vec![CanvasCell::default(); cols as usize * rows as usize],
            images: Vec::new(),
            redraw_after: None,
            text_input: false,
        }
    }

    fn index(&self, x: u16, y: u16) -> Option<usize> {
        (x < self.cols && y < self.rows).then(|| y as usize * self.cols as usize + x as usize)
    }

    /// The cell at (`x`, `y`), if inside.
    pub fn cell(&self, x: u16, y: u16) -> Option<&CanvasCell> {
        self.index(x, y).and_then(|i| self.cells.get(i))
    }

    /// The cell at (`x`, `y`) for changing, if inside. Writing a wide character by hand is on you;
    /// [`CanvasView::set_string`] keeps the halves together.
    pub fn cell_mut(&mut self, x: u16, y: u16) -> Option<&mut CanvasCell> {
        let i = self.index(x, y)?;
        self.cells.get_mut(i)
    }

    /// Blank the cell at (`x`, `y`) keeping its style (a half of a wide character that lost the
    /// other half).
    fn blank(&mut self, x: u16, y: u16) {
        if let Some(c) = self.cell_mut(x, y) {
            c.symbol = " ".to_string();
        }
    }

    /// Before the cells `x0..x1` of row `y` are overwritten, blank the neighbours that would be
    /// left as half of a wide character.
    fn break_wide(&mut self, y: u16, x0: u16, x1: u16) {
        if x1 <= x0 {
            return;
        }
        if x0 > 0 && self.cell(x0, y).is_some_and(CanvasCell::is_continuation) {
            self.blank(x0 - 1, y);
        }
        let last = x1 - 1;
        let right_is_cont = self.cell(x1, y).is_some_and(CanvasCell::is_continuation);
        let last_is_wide = self
            .cell(last, y)
            .is_some_and(|c| !c.is_continuation() && symbol_width(&c.symbol) >= 2);
        if right_is_cont && last_is_wide {
            self.blank(x1, y);
        }
    }

    /// Write `s` from (`x`, `y`) in `style` and return how many columns it took.
    ///
    /// Widths are display widths: a wide character takes two cells (the right one is a
    /// continuation), a zero-width character (a combining mark, a variation selector, a joiner and
    /// what it joins) is added to the cell before it, and a control character is skipped. The text
    /// is clipped at the right edge; a wide character that does not fit whole is not written at
    /// all. Overwriting one half of a wide character blanks the other half. Nothing is written
    /// outside the canvas, and a newline is not special (it is skipped like any control).
    pub fn set_string(&mut self, x: u16, y: u16, s: &str, style: Style) -> u16 {
        if y >= self.rows {
            return 0;
        }
        let mut cx = x as u32;
        // The cell that owns the last written character (the one a zero-width char joins).
        let mut owner: Option<u16> = None;
        // The width the owner's character took (1 or 2): what a joined mark must not change.
        let mut owner_w = 1u32;
        let mut after_joiner = false;
        for ch in s.chars() {
            let w = match char_width(ch) {
                Some(w) => w as u32,
                None => continue,
            };
            if w == 0 || after_joiner {
                // Joins the previous character. Nothing to join at the start: dropped. A character
                // that would make the cell more than one grapheme, longer than the limit, or a
                // different width from the columns the cell takes is dropped as well.
                let mut joined = false;
                if let Some(o) = owner {
                    if let Some(c) = self.cell_mut(o, y) {
                        let mut candidate = String::with_capacity(c.symbol.len() + ch.len_utf8());
                        candidate.push_str(&c.symbol);
                        candidate.push(ch);
                        if symbol_fault(&candidate).is_none()
                            && symbol_width(&candidate) as u32 == owner_w
                        {
                            c.symbol = candidate;
                            joined = true;
                        }
                    }
                }
                after_joiner = joined && ch == '\u{200D}';
                continue;
            }
            if cx + w > self.cols as u32 {
                break;
            }
            let at = cx as u16;
            self.break_wide(y, at, at + w as u16);
            for k in 0..w as u16 {
                if let Some(c) = self.cell_mut(at + k, y) {
                    c.symbol = if k == 0 {
                        ch.to_string()
                    } else {
                        String::new()
                    };
                    c.style = style;
                }
            }
            owner = Some(at);
            owner_w = w;
            after_joiner = false;
            cx += w;
        }
        (cx - x as u32) as u16
    }

    /// Set every cell of `rect` (clipped to the canvas) to a space in `style`. The halves of wide
    /// characters cut by the edge of `rect` are blanked.
    pub fn fill(&mut self, rect: CellRect, style: Style) {
        let x1 = (rect.x as u32 + rect.cols as u32).min(self.cols as u32) as u16;
        let y1 = (rect.y as u32 + rect.rows as u32).min(self.rows as u32) as u16;
        for y in rect.y..y1 {
            self.break_wide(y, rect.x, x1);
            for x in rect.x..x1 {
                if let Some(c) = self.cell_mut(x, y) {
                    c.symbol = " ".to_string();
                    c.style = style;
                }
            }
        }
    }

    /// Add a picture placement.
    pub fn image(mut self, image: CanvasImage) -> CanvasView {
        self.images.push(image);
        self
    }

    /// Add a picture placement to a canvas held by `&mut` (the same as [`CanvasView::image`] for
    /// code that builds the canvas in a loop or in a helper).
    pub fn push_image(&mut self, image: CanvasImage) {
        self.images.push(image);
    }

    /// Ask to be drawn again after `after`.
    pub fn redraw_after(mut self, after: Duration) -> CanvasView {
        self.redraw_after = Some(after);
        self
    }

    /// Ask for the keys (`true`) or not.
    pub fn text_input(mut self, on: bool) -> CanvasView {
        self.text_input = on;
        self
    }

    /// Whether the canvas is plain data the host may draw. This is the one contract; the host
    /// calls it and refuses a bad canvas with the issue's message. It requires:
    ///
    /// - the area within [`CANVAS_MAX_SIDE`] per side and [`CANVAS_MAX_CELLS`] in all, and
    ///   `cells.len() == cols * rows`;
    /// - every symbol empty (a continuation) or one grapheme of at most
    ///   [`CANVAS_MAX_SYMBOL_BYTES`] bytes, with no control character (C0, DEL, C1) and 1 or 2
    ///   columns wide; a 2-column symbol is followed by a continuation in the same row, and a
    ///   continuation follows nothing else;
    /// - at most [`CANVAS_MAX_PLACEMENTS`] placements using at most [`CANVAS_MAX_PICTURES`]
    ///   distinct ids, each non-empty, inside the canvas and not lazy;
    /// - [`ImageData::Rgba`] with `width * height * 4` bytes and sides from 1 to
    ///   [`CANVAS_MAX_RGBA_SIDE`] (also inside frames); [`ImageData::Frames`] of at most
    ///   [`CANVAS_MAX_FRAMES`] frames and not inside another `Frames`.
    ///
    /// The delays of an animation are not checked: the host raises a short delay.
    pub fn check(&self) -> Result<(), CanvasIssue> {
        if self.cols > CANVAS_MAX_SIDE
            || self.rows > CANVAS_MAX_SIDE
            || self.cols as usize * self.rows as usize > CANVAS_MAX_CELLS
        {
            return Err(CanvasIssue::AreaTooLarge {
                cols: self.cols,
                rows: self.rows,
            });
        }
        let expected = self.cols as usize * self.rows as usize;
        if self.cells.len() != expected {
            return Err(CanvasIssue::CellCount {
                expected,
                actual: self.cells.len(),
            });
        }
        if self.cols > 0 {
            for (y, row) in self.cells.chunks(self.cols as usize).enumerate() {
                self.check_row(y as u16, row)?;
            }
        }
        if self.images.len() > CANVAS_MAX_PLACEMENTS {
            return Err(CanvasIssue::TooManyPlacements {
                count: self.images.len(),
            });
        }
        let mut ids = std::collections::HashSet::new();
        for im in &self.images {
            ids.insert(im.id);
            if ids.len() > CANVAS_MAX_PICTURES {
                return Err(CanvasIssue::TooManyPictures { count: ids.len() });
            }
        }
        for (index, im) in self.images.iter().enumerate() {
            if im.cols == 0 || im.rows == 0 {
                return Err(CanvasIssue::ImageZeroSize { index });
            }
            if im.x as u32 + im.cols as u32 > self.cols as u32
                || im.y as u32 + im.rows as u32 > self.rows as u32
            {
                return Err(CanvasIssue::ImageOutOfBounds { index });
            }
            if let Some(data) = &im.image {
                check_image(data, index, false)?;
            }
        }
        Ok(())
    }

    fn check_row(&self, y: u16, row: &[CanvasCell]) -> Result<(), CanvasIssue> {
        // The x of a wide character still waiting for its continuation, and whether the previous
        // cell was a wide character (so an empty cell may follow).
        let mut pending: Option<u16> = None;
        for (x, cell) in row.iter().enumerate() {
            let x = x as u16;
            if cell.symbol.is_empty() {
                if pending.take().is_none() {
                    return Err(CanvasIssue::StrayContinuation { x, y });
                }
                continue;
            }
            if let Some(wx) = pending {
                return Err(CanvasIssue::WideWithoutContinuation { x: wx, y });
            }
            match symbol_fault(&cell.symbol) {
                None => {}
                Some(SymbolFault::Control) => return Err(CanvasIssue::CellControl { x, y }),
                Some(SymbolFault::TooLong) => return Err(CanvasIssue::CellSymbolTooLong { x, y }),
                Some(SymbolFault::NotOne) => return Err(CanvasIssue::CellNotOneGrapheme { x, y }),
                Some(SymbolFault::BadWidth) => return Err(CanvasIssue::CellBadWidth { x, y }),
            }
            if symbol_width(&cell.symbol) == 2 {
                pending = Some(x);
            }
        }
        if let Some(wx) = pending {
            return Err(CanvasIssue::WideWithoutContinuation { x: wx, y });
        }
        Ok(())
    }
}

/// The number of terminal columns a cell symbol takes. This is the one width function of the
/// canvas: [`CanvasView::check`], [`CanvasView::set_string`], the host's drawing and the ratatui
/// adapter all use it, so they cannot disagree.
///
/// It is the Unicode display width (`unicode-width`) plus one column for each halfwidth katakana
/// voiced or semi-voiced sound mark (U+FF9E, U+FF9F). `unicode-width` counts those marks as zero
/// (they extend the grapheme before them), but ratatui, which draws the screen, and the terminals
/// it targets give each mark its own column, so `"ﾃﾞ"` takes two. The result matches ratatui's own
/// cell width for every symbol a canvas cell may hold (a test in `konoma-plugin-ratatui` compares
/// them).
pub fn symbol_width(symbol: &str) -> usize {
    symbol.width()
        + symbol
            .chars()
            .filter(|c| is_halfwidth_sound_mark(*c))
            .count()
}

/// U+FF9E and U+FF9F.
fn is_halfwidth_sound_mark(c: char) -> bool {
    matches!(c, '\u{FF9E}' | '\u{FF9F}')
}

/// The columns one character takes by itself (`None` for a control character). A halfwidth sound
/// mark is a column of its own, as in [`symbol_width`].
fn char_width(c: char) -> Option<usize> {
    if is_halfwidth_sound_mark(c) {
        return Some(1);
    }
    c.width()
}

/// What is wrong with a non-empty cell symbol.
enum SymbolFault {
    Control,
    TooLong,
    NotOne,
    BadWidth,
}

/// Whether `symbol` (not empty) may sit in a canvas cell: one grapheme of at most
/// [`CANVAS_MAX_SYMBOL_BYTES`] bytes, no control character, 1 or 2 columns wide.
fn symbol_fault(symbol: &str) -> Option<SymbolFault> {
    // The common case: one printable ASCII character.
    if let [b] = symbol.as_bytes() {
        if (0x20..0x7f).contains(b) {
            return None;
        }
    }
    if symbol.len() > CANVAS_MAX_SYMBOL_BYTES {
        return Some(SymbolFault::TooLong);
    }
    if symbol.chars().any(char::is_control) {
        return Some(SymbolFault::Control);
    }
    if symbol.graphemes(true).nth(1).is_some() {
        return Some(SymbolFault::NotOne);
    }
    match symbol_width(symbol) {
        1 | 2 => None,
        _ => Some(SymbolFault::BadWidth),
    }
}

/// Check the pixels of one picture (`nested`: inside a `Frames`).
fn check_image(data: &ImageData, index: usize, nested: bool) -> Result<(), CanvasIssue> {
    match data {
        ImageData::Lazy(_) => Err(CanvasIssue::ImageLazy { index }),
        ImageData::Rgba {
            width,
            height,
            pixels,
        } => {
            if *width == 0 || *height == 0 {
                return Err(CanvasIssue::ImageRgbaSize { index });
            }
            if *width > CANVAS_MAX_RGBA_SIDE || *height > CANVAS_MAX_RGBA_SIDE {
                return Err(CanvasIssue::ImageTooLarge { index });
            }
            if pixels.len() as u64 != *width as u64 * *height as u64 * 4 {
                return Err(CanvasIssue::ImageRgbaSize { index });
            }
            Ok(())
        }
        ImageData::Frames(frames) => {
            if nested {
                return Err(CanvasIssue::ImageNestedFrames { index });
            }
            if frames.len() > CANVAS_MAX_FRAMES {
                return Err(CanvasIssue::ImageTooManyFrames { index });
            }
            frames
                .iter()
                .try_for_each(|(d, _)| check_image(d, index, true))
        }
        _ => Ok(()),
    }
}
