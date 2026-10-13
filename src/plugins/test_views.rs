//! Plugins the host's tests register to stand in for the standard ones: each returns the same
//! **display type** a standard plugin does, under an id of its own (see `test_plugins` for how a
//! test registers one). The host must draw them the way it draws the standard plugin, because it
//! chooses a screen from what a plugin declares and returns, never from its id. **Not part of the
//! product.**

use std::path::{Path, PathBuf};
use std::sync::Arc;

use konoma_plugin::prelude::*;
use konoma_plugin::view::{
    DocOptions, DocumentView, Grid, ImageData, ImageView, Layout, PageLayout, ReprDecl, TableView,
    TextSource, TextView,
};
use konoma_plugin::{Origin, Resources};

fn read(input: &Input) -> Result<(String, Option<PathBuf>), Failure> {
    match input {
        Input::File { path, .. } => Ok((std::fs::read_to_string(path)?, Some(path.clone()))),
        Input::Content { text, .. } => Ok((text.clone(), None)),
        Input::Embed { source, .. } => Ok((source.clone(), None)),
    }
}

fn manifest(id: &str, ext: &str, kind: ViewKind) -> Manifest {
    Manifest::new(id, "0.1.0")
        .bind(Bind::files([format!("*.{ext}")]))
        .shows(kind)
}

// ---- a document that is the file as it is (the Markdown plugin's shape) ------------------------

/// A document plugin of the Markdown plugin's shape: the file's text as a `Document`, and the
/// representations `rendered` / `source` (the source is the file itself). It also takes the content
/// type `markdown` when `takes_markdown` is set, to stand in for the standard plugin completely.
pub(crate) struct DocFile {
    pub id: &'static str,
    pub ext: &'static str,
    pub takes_markdown: bool,
}

impl Plugin for DocFile {
    fn manifest(&self) -> Manifest {
        let mut m = manifest(self.id, self.ext, ViewKind::Document);
        if self.takes_markdown {
            m = m.bind(Bind::content("markdown"));
        }
        m
    }

    fn open(&self, input: Input, _: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        let (text, path) = read(&input)?;
        let origin = match (&input, path.clone()) {
            (Input::File { .. }, Some(p)) => Origin::File(p),
            (Input::Content { origin, .. }, _) => origin.clone(),
            _ => Origin::File(PathBuf::new()),
        };
        let resources = match &input {
            Input::Content { resources, .. } => resources.clone(),
            _ => Resources::new(),
        };
        Ok(Box::new(DocFileSession {
            text,
            path,
            origin,
            resources,
        }))
    }
}

struct DocFileSession {
    text: String,
    path: Option<PathBuf>,
    origin: Origin,
    resources: Resources,
}

impl Session for DocFileSession {
    fn layout(&self) -> Layout {
        Layout {
            reprs: vec![
                ReprDecl::new("rendered", Tr::new("Rendered")),
                ReprDecl::new("source", Tr::new("Source")),
            ],
            default_repr: Some("rendered".into()),
            default_repr_without_pixels: None,
            pages: None,
        }
    }

    fn view(&mut self, req: &ViewReq, _: &ViewCx) -> Result<View, Failure> {
        if req.repr.as_deref() == Some("source") {
            let (source, file_lines) = match &self.path {
                Some(p) => (TextSource::File(p.clone()), true),
                None => (TextSource::Owned(self.text.clone()), false),
            };
            return Ok(View::Text(TextView {
                source,
                styler: None,
                file_lines,
                syntax_name: Some("markdown".into()),
            }));
        }
        Ok(View::Document(DocumentView {
            markdown: self.text.clone(),
            origin: self.origin.clone(),
            resources: self.resources.clone(),
            options: DocOptions::default(),
            title: None,
            truncated: false,
        }))
    }
}

// ---- a document converted from the file, handed over as Markdown (Word's shape) ----------------

/// A plugin that converts its file and answers "open this again as Markdown" (Word's shape): the
/// text of the file is the Markdown, and the host hands it to the plugin that takes `markdown`.
pub(crate) struct DocConverted {
    pub id: &'static str,
    pub ext: &'static str,
}

impl Plugin for DocConverted {
    fn manifest(&self) -> Manifest {
        manifest(self.id, self.ext, ViewKind::Document).converts()
    }

    fn open(&self, input: Input, _: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        let (text, path) = read(&input)?;
        Ok(Box::new(Converted {
            text,
            from: path.unwrap_or_default(),
        }))
    }
}

struct Converted {
    text: String,
    from: PathBuf,
}

impl Session for Converted {
    fn view(&mut self, _: &ViewReq, _: &ViewCx) -> Result<View, Failure> {
        Ok(View::Reopen(konoma_plugin::view::Reopen {
            content_type: "markdown".into(),
            text: self.text.clone(),
            resources: Resources::new(),
            origin: Origin::Converted {
                from: self.from.clone(),
            },
            truncated: false,
        }))
    }
}

// ---- a deck: one long document whose pages start at headings (the presentation's shape) -------

/// A plugin of the presentation's shape: a `Document` converted from the file whose pages start at
/// its `#` headings (`PageLayout::Anchored`), with the representations `text` and `picture`.
pub(crate) struct Deck {
    pub id: &'static str,
    pub ext: &'static str,
}

impl Plugin for Deck {
    fn manifest(&self) -> Manifest {
        manifest(self.id, self.ext, ViewKind::Document).converts()
    }

    fn open(&self, input: Input, _: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        let (text, path) = read(&input)?;
        let slides = text.lines().filter(|l| l.starts_with("# ")).count();
        Ok(Box::new(DeckSession {
            text,
            slides,
            from: path.unwrap_or_default(),
        }))
    }
}

struct DeckSession {
    text: String,
    slides: usize,
    from: PathBuf,
}

impl Session for DeckSession {
    fn layout(&self) -> Layout {
        Layout {
            // The names a presentation plugin uses: the host's `R` on a deck switches between them.
            reprs: vec![
                ReprDecl::new("picture", Tr::new("Pictures")),
                ReprDecl::new("text", Tr::new("Text")),
            ],
            default_repr: Some("text".into()),
            default_repr_without_pixels: Some("text".into()),
            pages: Some(PageLayout::Anchored {
                heading_level: 1,
                count: self.slides,
            }),
        }
    }

    fn view(&mut self, _: &ViewReq, _: &ViewCx) -> Result<View, Failure> {
        Ok(View::Document(DocumentView {
            markdown: self.text.clone(),
            origin: Origin::Converted {
                from: self.from.clone(),
            },
            resources: Resources::new(),
            options: DocOptions {
                front_matter: false,
                tasks_writable: false,
                ..DocOptions::default()
            },
            title: None,
            truncated: false,
        }))
    }
}

// ---- a table with pages (the spreadsheet's shape) and a plain table (the CSV's shape) -----------

struct Cells {
    rows: Vec<Vec<String>>,
    ncols: usize,
    /// The first row names the columns (a CSV); otherwise the columns are lettered (a sheet).
    first_row_is_header: bool,
}

impl Grid for Cells {
    fn nrows(&self) -> usize {
        self.rows
            .len()
            .saturating_sub(usize::from(self.first_row_is_header))
    }
    fn ncols(&self) -> usize {
        self.ncols
    }
    fn header(&self, col: usize) -> Option<String> {
        if self.first_row_is_header {
            self.rows.first().and_then(|r| r.get(col)).cloned()
        } else {
            Some(((b'A' + (col % 26) as u8) as char).to_string())
        }
    }
    fn cell(&self, row: usize, col: usize) -> String {
        let row = row + usize::from(self.first_row_is_header);
        self.rows
            .get(row)
            .and_then(|r| r.get(col))
            .cloned()
            .unwrap_or_default()
    }
}

fn cells(text: &str, first_row_is_header: bool) -> Cells {
    let rows: Vec<Vec<String>> = text
        .lines()
        .map(|l| l.split(',').map(str::to_string).collect())
        .collect();
    let ncols = rows.iter().map(Vec::len).max().unwrap_or(0);
    Cells {
        rows,
        ncols,
        first_row_is_header,
    }
}

/// A table plugin. With `sheets > 1` it has separate pages (a page is the lines of the file
/// between `---` lines) and converts the file, like the spreadsheet plugin; with one it is the CSV
/// plugin's shape (the whole file, commas between cells).
pub(crate) struct TableLike {
    pub id: &'static str,
    pub ext: &'static str,
    pub paged: bool,
    /// Declares an exclusive work group (the Office plugins do): reading it is heavy, so the host
    /// reads it again only when the file changed.
    pub heavy: bool,
    /// Counts the `open`s.
    pub opens: Arc<std::sync::atomic::AtomicUsize>,
}

impl TableLike {
    pub(crate) fn new(id: &'static str, ext: &'static str, paged: bool) -> TableLike {
        TableLike {
            id,
            ext,
            paged,
            heavy: false,
            opens: Arc::default(),
        }
    }
}

impl Plugin for TableLike {
    fn manifest(&self) -> Manifest {
        let mut m = manifest(self.id, self.ext, ViewKind::Table);
        if self.heavy {
            m = m.work_group("test-heavy");
        }
        if self.paged {
            m.converts()
        } else {
            m.unavailable(konoma_plugin::Unavailable::RawText { with_reason: false })
        }
    }

    fn open(&self, input: Input, _: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        self.opens.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let (text, _) = read(&input)?;
        let pages: Vec<String> = if self.paged {
            text.split("\n---\n").map(str::to_string).collect()
        } else {
            vec![text]
        };
        Ok(Box::new(TableSession {
            pages,
            paged: self.paged,
        }))
    }
}

struct TableSession {
    pages: Vec<String>,
    paged: bool,
}

impl Session for TableSession {
    fn layout(&self) -> Layout {
        Layout {
            pages: self.paged.then(|| PageLayout::Separate {
                count: Some(self.pages.len()),
                titles: (1..=self.pages.len())
                    .map(|i| format!("Sheet{i}"))
                    .collect(),
            }),
            ..Layout::single()
        }
    }

    fn view(&mut self, req: &ViewReq, _: &ViewCx) -> Result<View, Failure> {
        let page = req
            .page
            .unwrap_or(0)
            .min(self.pages.len().saturating_sub(1));
        let text = self.pages.get(page).cloned().unwrap_or_default();
        Ok(View::Table(TableView {
            grid: Arc::new(cells(&text, !self.paged)),
            title: None,
            rainbow: true,
            addressed: self.paged,
        }))
    }
}

// ---- a picture, with pages (the PDF's shape) or a vector one (the SVG's shape) -----------------

/// What a picture plugin returns.
#[derive(Clone, Copy)]
pub(crate) enum Pic {
    /// Raw pixels, one flat color per page (red, green, blue), `pages` pages.
    Pages(usize),
    /// An SVG that claims to be trusted, with no base directory.
    ClaimedTrustedSvg,
    /// An SVG that claims to be trusted, with a base directory (the file's own).
    ClaimedTrustedSvgInDir,
}

pub(crate) struct Picture {
    pub id: &'static str,
    pub ext: &'static str,
    pub pic: Pic,
}

/// The SVG every vector test plugin returns: a red square of 40 by 40.
pub(crate) const RED_SQUARE: &str = "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"40\" height=\"40\"><rect width=\"40\" height=\"40\" fill=\"#ff0000\"/></svg>";

impl Plugin for Picture {
    fn manifest(&self) -> Manifest {
        manifest(self.id, self.ext, ViewKind::Image)
    }

    fn open(&self, input: Input, _: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        let path = match &input {
            Input::File { path, .. } => path.clone(),
            _ => PathBuf::new(),
        };
        Ok(Box::new(PictureSession {
            pic: self.pic,
            path,
        }))
    }
}

struct PictureSession {
    pic: Pic,
    path: PathBuf,
}

impl Session for PictureSession {
    fn layout(&self) -> Layout {
        match self.pic {
            Pic::Pages(n) => Layout {
                pages: Some(PageLayout::Separate {
                    count: Some(n),
                    titles: Vec::new(),
                }),
                ..Layout::single()
            },
            _ => Layout::single(),
        }
    }

    fn view(&mut self, req: &ViewReq, _: &ViewCx) -> Result<View, Failure> {
        match self.pic {
            Pic::Pages(n) => {
                let page = req.page.unwrap_or(0).min(n.saturating_sub(1));
                let color: [u8; 4] = match page % 3 {
                    0 => [255, 0, 0, 255],
                    1 => [0, 255, 0, 255],
                    _ => [0, 0, 255, 255],
                };
                let pixels: Vec<u8> = color.iter().copied().cycle().take(4 * 8 * 8).collect();
                Ok(View::Image(ImageView::new(ImageData::Rgba {
                    width: 8,
                    height: 8,
                    pixels: Arc::new(pixels),
                })))
            }
            Pic::ClaimedTrustedSvg | Pic::ClaimedTrustedSvgInDir => {
                let base_dir = matches!(self.pic, Pic::ClaimedTrustedSvgInDir)
                    .then(|| self.path.parent().map(Path::to_path_buf))
                    .flatten();
                Ok(View::Image(ImageView {
                    data: ImageData::Svg {
                        data: Arc::from(RED_SQUARE.as_bytes()),
                        base_dir,
                        trusted: true,
                    },
                    sharp_zoom: true,
                    raster_max_px: None,
                }))
            }
        }
    }
}

// ---- text the plugin colors itself ----------------------------------------------------------------

/// A text plugin whose lines are colored by the plugin (a `Line` with a red span in it), not the file
/// itself: the host windows the text and draws these colors.
pub(crate) struct Styled {
    pub id: &'static str,
    pub ext: &'static str,
}

impl Plugin for Styled {
    fn manifest(&self) -> Manifest {
        manifest(self.id, self.ext, ViewKind::Text)
    }

    fn open(&self, input: Input, _: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        let (text, _) = read(&input)?;
        Ok(Box::new(StyledSession { text }))
    }
}

struct StyledSession {
    text: String,
}

impl Session for StyledSession {
    fn view(&mut self, _: &ViewReq, _: &ViewCx) -> Result<View, Failure> {
        use konoma_plugin::style::{Color, Line, Span, Style};
        Ok(View::text(self.text.lines().map(
            |l| match l.strip_prefix("!! ") {
                Some(rest) => {
                    Line::from(Span::styled(rest.to_string(), Style::new().fg(Color::Red)))
                }
                None => Line::raw(l),
            },
        )))
    }
}

// ---- an embedded kind drawn as an SVG that claims to be trusted ----------------------------------

/// A plugin that takes the embedded kind `fence:mermaid` (a user's plugin wins over the standard
/// one) and answers every fence with the red square, claiming it is trusted.
pub(crate) struct EmbedSvg;

impl Plugin for EmbedSvg {
    fn manifest(&self) -> Manifest {
        Manifest::new("x-embed-svg", "0.1.0").bind(Bind::embed("fence:mermaid"))
    }

    fn open(&self, _: Input, _: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        Ok(Box::new(EmbedSvgSession))
    }
}

struct EmbedSvgSession;

impl Session for EmbedSvgSession {
    fn view(&mut self, _: &ViewReq, _: &ViewCx) -> Result<View, Failure> {
        Ok(View::Image(ImageView::new(ImageData::Svg {
            data: Arc::from(RED_SQUARE.as_bytes()),
            base_dir: None,
            trusted: true,
        })))
    }
}

// ---- a plugin that cannot show the file ----------------------------------------------------------

/// A table plugin whose `open` always fails; what the host shows instead is the plugin's
/// `Manifest::unavailable`.
pub(crate) struct Broken {
    pub id: &'static str,
    pub ext: &'static str,
    pub how: konoma_plugin::Unavailable,
}

impl Plugin for Broken {
    fn manifest(&self) -> Manifest {
        manifest(self.id, self.ext, ViewKind::Table).unavailable(self.how.clone())
    }

    fn open(&self, _: Input, _: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        Err(Failure::corrupt(Tr::new("it is broken beyond repair")))
    }
}

// ---- a plugin that answers with a notice ---------------------------------------------------------

/// A plugin that declares nothing and answers every file with a `Message` view.
pub(crate) struct Notifier {
    pub id: &'static str,
    pub ext: &'static str,
}

impl Plugin for Notifier {
    fn manifest(&self) -> Manifest {
        Manifest::new(self.id, "0.1.0").bind(Bind::files([format!("*.{}", self.ext)]))
    }

    fn open(&self, _: Input, _: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        Ok(Box::new(NotifierSession))
    }
}

struct NotifierSession;

impl Session for NotifierSession {
    fn view(&mut self, _: &ViewReq, _: &ViewCx) -> Result<View, Failure> {
        Ok(View::Message(konoma_plugin::view::MessageView {
            lines: vec![
                konoma_plugin::style::Line::raw("Nothing to see here."),
                konoma_plugin::style::Line::raw("Come back later."),
            ],
        }))
    }
}

// ---- a plugin with several representations of what it shows (`R` walks them) ---------------------

/// What a [`Multi`] plugin shows.
#[derive(Clone, Copy)]
pub(crate) enum Shape {
    /// A document with three slide headings (pages that start at headings).
    Doc,
    /// A table; with pages when `pages > 1`.
    Table,
    /// A picture; with pages when `pages > 1`.
    Image,
    /// Lines of text the plugin makes.
    Text,
}

/// The ids of the representations a [`Multi`] plugin declares, in order.
pub(crate) const MULTI_REPRS: [&str; 3] = ["alpha", "beta", "gamma"];

/// A plugin that declares `reprs` representations (`alpha`, `beta`, `gamma`; labels `Alpha`, ...)
/// and answers each with a view of its shape that says which one it is, under an id of its own.
pub(crate) struct Multi {
    pub id: &'static str,
    pub ext: &'static str,
    pub shape: Shape,
    pub reprs: usize,
    pub pages: usize,
}

impl Plugin for Multi {
    fn manifest(&self) -> Manifest {
        let kind = match self.shape {
            Shape::Doc => ViewKind::Document,
            Shape::Table => ViewKind::Table,
            Shape::Image => ViewKind::Image,
            Shape::Text => ViewKind::Text,
        };
        let m = manifest(self.id, self.ext, kind);
        // A document whose pages start at headings is converted from its file, like a deck.
        if matches!(self.shape, Shape::Doc) {
            m.converts()
        } else {
            m
        }
    }

    fn open(&self, input: Input, _: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        let path = match &input {
            Input::File { path, .. } => path.clone(),
            _ => PathBuf::new(),
        };
        Ok(Box::new(MultiSession {
            shape: self.shape,
            reprs: self.reprs,
            pages: self.pages,
            path,
        }))
    }
}

struct MultiSession {
    shape: Shape,
    reprs: usize,
    pages: usize,
    path: PathBuf,
}

impl Session for MultiSession {
    fn layout(&self) -> Layout {
        let labels = [Tr::new("Alpha"), Tr::new("Beta"), Tr::new("Gamma")];
        let reprs: Vec<ReprDecl> = if self.reprs == 0 {
            Vec::new()
        } else {
            (0..self.reprs.min(3))
                .map(|i| ReprDecl::new(MULTI_REPRS[i], labels[i].clone()))
                .collect()
        };
        let pages = match self.shape {
            Shape::Doc => Some(PageLayout::Anchored {
                heading_level: 1,
                count: self.pages.max(1),
            }),
            Shape::Table | Shape::Image if self.pages > 1 => Some(PageLayout::Separate {
                count: Some(self.pages),
                titles: Vec::new(),
            }),
            _ => None,
        };
        Layout {
            reprs,
            default_repr: Some("alpha".into()),
            default_repr_without_pixels: Some("alpha".into()),
            pages,
        }
    }

    fn view(&mut self, req: &ViewReq, _: &ViewCx) -> Result<View, Failure> {
        let repr = req.repr.clone().unwrap_or_else(|| "alpha".into());
        let idx = MULTI_REPRS.iter().position(|r| *r == repr).unwrap_or(0);
        let page = req.page.unwrap_or(0).min(self.pages.max(1) - 1);
        Ok(match self.shape {
            Shape::Doc => View::Document(DocumentView {
                markdown: (1..=self.pages.max(1))
                    .map(|n| {
                        format!(
                            "# Slide {n}\n\nthis is {repr}\n\n{}",
                            "filler\n\n".repeat(40)
                        )
                    })
                    .collect(),
                origin: Origin::File(self.path.clone()),
                resources: Resources::new(),
                options: DocOptions::default(),
                title: None,
                truncated: false,
            }),
            Shape::Table => View::Table(TableView {
                grid: Arc::new(Cells {
                    rows: vec![
                        vec!["shown".into(), "page".into()],
                        vec![repr, format!("p{}", page + 1)],
                    ],
                    ncols: 2,
                    first_row_is_header: true,
                }),
                title: None,
                rainbow: true,
                addressed: false,
            }),
            Shape::Image => {
                // Red says the representation, green says the page.
                let color = [(idx * 100 + 55) as u8, (page * 80) as u8, 0, 255];
                let pixels: Vec<u8> = color.iter().copied().cycle().take(4 * 8 * 8).collect();
                View::Image(ImageView::new(ImageData::Rgba {
                    width: 8,
                    height: 8,
                    pixels: Arc::new(pixels),
                }))
            }
            Shape::Text => View::text([format!("this is {repr}")]),
        })
    }
}
