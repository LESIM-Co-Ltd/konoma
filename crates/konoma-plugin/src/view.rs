//! The display types a plugin returns. The host draws them with its own widgets.

use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crate::input::{LazyResource, Origin, Resources};
use crate::manifest::ViewKind;
use crate::style::Line;
use crate::text::Tr;

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
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ViewReq {
    /// The representation (`None` = the default).
    pub repr: Option<String>,
    /// The page, counted from 0 (`None` = the first or the whole). A page past the end is clamped
    /// to the last one.
    pub page: Option<usize>,
    /// The size of the area in cells.
    pub cols: u16,
    pub rows: u16,
    /// The size of one cell in pixels, if known.
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
}

impl Default for ViewReq {
    fn default() -> ViewReq {
        ViewReq::new(80, 24)
    }
}

/// A display type.
#[derive(Debug)]
pub enum View {
    Text(TextView),
    Document(DocumentView),
    Table(TableView),
    Image(ImageView),
    List(ListView),
    Diff(DiffView),
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
