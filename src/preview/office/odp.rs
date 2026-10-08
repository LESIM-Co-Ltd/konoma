//! OpenDocument presentation preview (odp / otp): the reading side. The same output as the
//! PowerPoint reader (`pptx.rs`: one `##` section per slide, then the shapes in reading order, then
//! the speaker notes as a quote) built on the OpenDocument text reader's walk (`odt.rs`): the
//! paragraphs, lists, tables, pictures and formulas of a text box are read by the same code as the
//! ones of a Writer document, and the order of the shapes is `slide_order::reading_order`.
//!
//! This is a child module of `odt.rs` (it uses the private walk) and so of `docx.rs` (the writer).
//!
//! # What is converted
//!
//! | OpenDocument | Markdown |
//! |---|---|
//! | `draw:page` of `office:presentation`, in order | `## Slide N: title` |
//! | the page's style (`style:family="drawing-page"`, through `style:parent-style-name`) has `presentation:visibility="hidden"` | the heading ends with ` (hidden)` |
//! | the first `draw:frame` with `presentation:class="title"` that has text | the heading; any other title is text |
//! | `draw:frame` > `draw:text-box` (`presentation:class` outline / subtitle / text ..) | its paragraphs and lists, as in Writer |
//! | `draw:frame` > `table:table` | GFM table (the preview picture LibreOffice stores beside it is not shown) |
//! | `draw:frame` > `draw:image` | `![alt](office-img://..)` |
//! | `draw:frame` > `draw:object` of a formula (MathML) | `$latex$` / `$$latex$$` |
//! | `draw:frame` > `draw:object` of a chart | `[chart: title]` |
//! | `draw:custom-shape`, `draw:rect` .. that hold text | their paragraphs |
//! | `draw:g` | its members, as one block placed at their bounding rectangle |
//! | `presentation:notes` > the frame of `presentation:class="notes"` | a quote block under the slide |
//! | `text:page-number` in a text | the slide's number |
//! | header, footer, date and page number frames, page thumbnails, connectors, lines, comments | nothing |
//!
//! Positions are `svg:x` / `svg:y` / `svg:width` / `svg:height` (cm, mm, in, pt, pc, px -> EMU,
//! the unit of `slide_order`); a `draw:transform` (rotate, translate, scale, skew, matrix) is
//! applied to the four corners and the shape is placed at their bounding box. A frame with no
//! position of its own takes the one of the frame with the same `presentation:class` on the
//! slide's master page (`style:master-page` of `styles.xml`).
//!
//! **Invariant** as for PowerPoint: the Markdown holds one level-2 heading per slide, in order, and
//! no other (a `text:h` in a slide is a paragraph).
//!
//! # Safety
//!
//! Same as the Writer reader and the PowerPoint one: the package goes through `container`, every XML
//! part through `XmlReader` (depth 256), the slides are read **one at a time** (a slide is one tree
//! under the node and text budgets of [`DocOptions`], dropped when written) and the budgets for
//! slides, shapes per slide, pictures, table cells (repeat attributes included), formulas and the
//! output apply. Cancellation is checked per slide and per shape.

use std::collections::HashMap;

use super::super::pptx::{write_heading, write_notes_quote, SlideInfo};
use super::*;
use crate::preview::office::slide_order::{bounding, reading_order, Kind, Rect, Shape};

/// Longest slide title kept (characters).
const TITLE_CHARS: usize = 200;
/// Deepest group nesting followed.
const MAX_GROUP_DEPTH: usize = 20;
/// Most master pages read.
const MAX_MASTERS: usize = 1_000;
/// Most nodes / bytes of one `style:page-layout` read (a real one is a handful of elements).
const PAGE_LAYOUT_NODES: usize = 1_000;
const PAGE_LAYOUT_BYTES: usize = 1024 * 1024;
/// Most frames of the notes page read.
const MAX_NOTE_FRAMES: usize = 8;
/// Most bytes of a chart object's `content.xml` read to find its title.
const CHART_PART_BYTES: u64 = 512 * 1024;
/// Largest absolute coordinate kept (EMU); the arithmetic of the order stays far from overflow.
const LIM: f64 = (1u64 << 40) as f64;

// ---------------------------------------------------------------------------------------------
// lengths and positions
// ---------------------------------------------------------------------------------------------

/// An ODF length (`2.5cm`, `10mm`, `1in`, `12pt`, `1pc`, `96px`) in EMU (914,400 to the inch).
fn emu(v: &str) -> Option<f64> {
    let v = v.trim();
    let end = v
        .find(|c: char| !(c.is_ascii_digit() || matches!(c, '.' | '-' | '+')))
        .unwrap_or(v.len());
    let (num, unit) = v.split_at(end);
    let n: f64 = num.parse().ok()?;
    let per = match unit.trim() {
        "cm" => 360_000.0,
        "mm" => 36_000.0,
        "in" => 914_400.0,
        "pt" => 12_700.0,
        "pc" => 152_400.0,
        "px" => 9_525.0,
        _ => return None,
    };
    let e = n * per;
    e.is_finite().then_some(e.clamp(-LIM, LIM))
}

fn len_attr(n: &Node, name: &str) -> Option<f64> {
    n.attr(name).and_then(emu)
}

/// A number of an SVG transform (`translate (2cm 1cm)` takes lengths, the rest plain numbers).
fn plain_number(s: &str) -> Option<f64> {
    let n: f64 = s.trim().parse().ok()?;
    n.is_finite().then_some(n)
}

/// The transform matrix `[a b c d e f]` of a `draw:transform` list (SVG semantics: the list is the
/// product of its operations, the last one applies to the point first). `None` for anything this
/// does not understand.
fn transform_of(list: &str) -> Option<[f64; 6]> {
    fn mul(m: [f64; 6], n: [f64; 6]) -> [f64; 6] {
        [
            m[0] * n[0] + m[2] * n[1],
            m[1] * n[0] + m[3] * n[1],
            m[0] * n[2] + m[2] * n[3],
            m[1] * n[2] + m[3] * n[3],
            m[0] * n[4] + m[2] * n[5] + m[4],
            m[1] * n[4] + m[3] * n[5] + m[5],
        ]
    }
    let mut m = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];
    let mut rest = list.trim();
    let mut ops = 0;
    while !rest.is_empty() {
        ops += 1;
        if ops > 16 {
            return None;
        }
        let open = rest.find('(')?;
        let close = rest.find(')')?;
        if close < open {
            return None;
        }
        let name = rest[..open].trim().trim_start_matches(',').trim();
        let args: Vec<&str> = rest[open + 1..close]
            .split(|c: char| c.is_whitespace() || c == ',')
            .filter(|a| !a.is_empty())
            .collect();
        let op = match (name, args.len()) {
            ("translate", 1 | 2) => {
                let tx = emu(args[0])?;
                let ty = args.get(1).map_or(Some(0.0), |a| emu(a))?;
                [1.0, 0.0, 0.0, 1.0, tx, ty]
            }
            ("scale", 1 | 2) => {
                let sx = plain_number(args[0])?;
                let sy = args.get(1).map_or(Some(sx), |a| plain_number(a))?;
                [sx, 0.0, 0.0, sy, 0.0, 0.0]
            }
            ("rotate", 1) => {
                let (s, c) = plain_number(args[0])?.sin_cos();
                [c, s, -s, c, 0.0, 0.0]
            }
            ("skewX", 1) => [1.0, 0.0, plain_number(args[0])?.tan(), 1.0, 0.0, 0.0],
            ("skewY", 1) => [1.0, plain_number(args[0])?.tan(), 0.0, 1.0, 0.0, 0.0],
            ("matrix", 6) => [
                plain_number(args[0])?,
                plain_number(args[1])?,
                plain_number(args[2])?,
                plain_number(args[3])?,
                emu(args[4])?,
                emu(args[5])?,
            ],
            _ => return None,
        };
        m = mul(m, op);
        rest = rest[close + 1..].trim_start_matches([',', ' ', '\t', '\n', '\r']);
    }
    m.iter().all(|v| v.is_finite()).then_some(m)
}

/// The rectangle a shape element states: `svg:x` `svg:y` `svg:width` `svg:height`, with its
/// `draw:transform` applied to the corners (a rotated shape is its bounding box). `None` when it
/// states no position.
fn own_rect(n: &Node) -> Option<Rect> {
    let (w, h) = (len_attr(n, "width")?, len_attr(n, "height")?);
    let m = n.attr("transform").and_then(transform_of);
    // A transform places the shape (LibreOffice writes `rotate (a) translate (x y)` and no
    // `svg:x`); without one the position must be stated.
    let (x, y) = match (len_attr(n, "x"), len_attr(n, "y"), m.is_some()) {
        (Some(x), Some(y), _) => (x, y),
        (None, None, true) => (0.0, 0.0),
        _ => return None,
    };
    let corners = [(x, y), (x + w, y), (x, y + h), (x + w, y + h)];
    let pts: Vec<(f64, f64)> = match m {
        Some(m) => corners
            .iter()
            .map(|&(px, py)| (m[0] * px + m[2] * py + m[4], m[1] * px + m[3] * py + m[5]))
            .collect(),
        None => corners.to_vec(),
    };
    if pts.iter().any(|(a, b)| !a.is_finite() || !b.is_finite()) {
        return None;
    }
    let min = |f: fn(&(f64, f64)) -> f64| pts.iter().map(f).fold(f64::INFINITY, f64::min);
    let max = |f: fn(&(f64, f64)) -> f64| pts.iter().map(f).fold(f64::NEG_INFINITY, f64::max);
    let (x0, y0, x1, y1) = (min(|p| p.0), min(|p| p.1), max(|p| p.0), max(|p| p.1));
    Some(Rect::new(
        x0.round() as i64,
        y0.round() as i64,
        (x1 - x0).round() as i64,
        (y1 - y0).round() as i64,
    ))
}

// ---------------------------------------------------------------------------------------------
// shapes
// ---------------------------------------------------------------------------------------------

/// What a shape shows.
enum Body<'n> {
    /// A `draw:frame`: text box, picture, table, formula, chart.
    Frame(&'n Node),
    /// A drawing shape that holds text (`draw:custom-shape` ..).
    Shape(&'n Node),
    /// A group, ordered inside.
    Group(Vec<Sh<'n>>),
}

struct Sh<'n> {
    kind: Kind,
    rect: Option<Rect>,
    body: Body<'n>,
}

/// The frames of the slide's master page by `presentation:class`.
type Master = HashMap<String, Rect>;

struct Build<'a> {
    master: Option<&'a Master>,
    shapes: usize,
    max_shapes: usize,
    truncated: bool,
}

/// The classes that are not the slide's content (the footer line of a slide).
fn is_furniture(class: Option<&str>) -> bool {
    matches!(
        class,
        Some("header" | "footer" | "date-time" | "page-number")
    )
}

fn kind_of(class: Option<&str>) -> Kind {
    match class {
        Some("title") => Kind::Title,
        Some("subtitle") => Kind::Subtitle,
        _ => Kind::Other,
    }
}

/// Whether `n` holds anything to show: text, a picture, an object, a table. A text box left empty
/// (an unfilled placeholder) does not.
fn has_content(n: &Node, depth: usize) -> bool {
    if depth > 40 {
        return false;
    }
    n.kids.iter().any(|k| match k {
        Kid::T(t) => !t.trim().is_empty(),
        Kid::N(c) => match c.name.as_str() {
            "image" | "object" | "object-ole" | "table" => true,
            "annotation" | "annotation-end" | "page-thumbnail" | "enhanced-geometry" => false,
            "title" | "desc" if c.prefix != "text" => false,
            _ => has_content(c, depth + 1),
        },
    })
}

fn build<'n>(
    nodes: impl Iterator<Item = &'n Node>,
    b: &mut Build<'_>,
    depth: usize,
    out: &mut Vec<Sh<'n>>,
) {
    for n in nodes {
        if b.shapes >= b.max_shapes {
            b.truncated = true;
            return;
        }
        let class = n.attr("class");
        match n.name.as_str() {
            "frame" => {
                if is_furniture(class) || !has_content(n, 0) {
                    continue;
                }
                b.shapes += 1;
                // A placeholder with no position of its own is where the master puts that class.
                let rect = own_rect(n)
                    .or_else(|| class.and_then(|c| b.master.and_then(|m| m.get(c)).copied()));
                out.push(Sh {
                    kind: kind_of(class),
                    rect,
                    body: Body::Frame(n),
                });
            }
            "g" | "a" if n.prefix == "draw" => {
                if depth >= MAX_GROUP_DEPTH {
                    b.truncated = true;
                    continue;
                }
                let mut kids = Vec::new();
                build(n.nodes(), b, depth + 1, &mut kids);
                if kids.is_empty() {
                    continue;
                }
                if n.name == "a" {
                    // A hyperlink around shapes: the shapes themselves.
                    out.append(&mut kids);
                    continue;
                }
                let rect = bounding(kids.iter().filter_map(|k| k.rect));
                out.push(Sh {
                    kind: Kind::Other,
                    rect,
                    body: Body::Group(kids),
                });
            }
            name if SHAPES.contains(&name) => {
                if is_furniture(class) || !has_content(n, 0) {
                    continue;
                }
                b.shapes += 1;
                out.push(Sh {
                    kind: kind_of(class),
                    rect: own_rect(n),
                    body: Body::Shape(n),
                });
            }
            _ => {}
        }
    }
}

/// The text of a title as one line.
fn plain_text(n: &Node, out: &mut String, depth: usize) {
    if depth > 32 || out.len() > 4096 {
        return;
    }
    for k in &n.kids {
        match k {
            Kid::T(t) => out.push_str(t),
            Kid::N(c) => match c.name.as_str() {
                "s" | "tab" | "line-break" => out.push(' '),
                "annotation" | "annotation-end" | "note" | "ruby-text" | "hidden-text"
                | "hidden-paragraph" => {}
                "p" | "h" | "list-item" => {
                    plain_text(c, out, depth + 1);
                    out.push(' ');
                }
                _ => plain_text(c, out, depth + 1),
            },
        }
    }
}

fn plain_title(n: &Node) -> String {
    let mut s = String::new();
    match n.name.as_str() {
        "frame" => {
            for tb in n.nodes().filter(|k| k.name == "text-box") {
                plain_text(tb, &mut s, 0);
            }
        }
        _ => plain_text(n, &mut s, 0),
    }
    clean(&s)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(TITLE_CHARS)
        .collect()
}

// ---------------------------------------------------------------------------------------------
// the package
// ---------------------------------------------------------------------------------------------

/// What `styles.xml` says about the master pages.
#[derive(Default)]
struct Masters {
    /// The frames of each master page, by page name.
    frames: HashMap<String, Master>,
    /// The size of the slides of each master page (its page layout's `fo:page-width` /
    /// `fo:page-height`), by page name.
    size: HashMap<String, Rect>,
}

/// The size of each `style:page-layout` among the children of the `office:automatic-styles` just
/// opened, by name, read through the end of that element. Every other child is skipped unread; a
/// layout over its budget is left out. Fails only when the XML is damaged.
fn read_page_layouts(
    rd: &mut XmlReader<impl BufRead>,
    layouts: &mut HashMap<String, Rect>,
) -> Result<(), OfficeError> {
    let mut buf = Vec::new();
    loop {
        buf.clear();
        let (e, empty) = match rd.read_event_into(&mut buf).map_err(xml_err)? {
            Event::Start(e) => (e.into_owned(), false),
            Event::Empty(e) => (e.into_owned(), true),
            Event::End(_) => return Ok(()),
            Event::Eof => {
                return Err(OfficeError::Corrupt(
                    "xml: unexpected end of document".into(),
                ))
            }
            _ => continue,
        };
        if e.local_name().as_ref() != b"page-layout" {
            if !empty {
                skip_rest(rd)?;
            }
            continue;
        }
        let mut budget = Budget::odf(PAGE_LAYOUT_NODES, PAGE_LAYOUT_BYTES);
        let Tree::Ok(l) = read_element(rd, &e, empty, &mut budget)? else {
            continue;
        };
        let size = l
            .child("page-layout-properties")
            .and_then(|p| Some((len_attr(p, "page-width")?, len_attr(p, "page-height")?)));
        // (A size of zero or less is no size: it would put every shape off the slide.)
        if let (Some(name), Some((w, h))) =
            (l.attr("name"), size.filter(|&(w, h)| w > 0.0 && h > 0.0))
        {
            if layouts.len() < MAX_MASTERS {
                layouts.insert(name.to_string(), Rect::new(0, 0, w as i64, h as i64));
            }
        }
    }
}

/// The frames and the page size of each master page of `styles.xml` (`office:master-styles`, and
/// the `style:page-layout`s of its automatic styles). A damaged part costs the inherited positions,
/// not the presentation.
fn read_masters(src: impl BufRead) -> Masters {
    let mut layouts: HashMap<String, Rect> = HashMap::new();
    let mut out = Masters::default();
    let mut rd = XmlReader::new(src);
    let mut buf = Vec::new();
    let mut root = false;
    loop {
        buf.clear();
        let Ok(ev) = rd.read_event_into(&mut buf) else {
            return out;
        };
        let (e, empty) = match ev {
            Event::Start(e) => (e.into_owned(), false),
            Event::Empty(e) => (e.into_owned(), true),
            Event::Eof => return out,
            _ => continue,
        };
        if !root {
            root = true;
            continue;
        }
        if e.local_name().as_ref() == b"automatic-styles" {
            // Only the page layouts matter (the size of a slide), and each one is read on its own
            // under a small budget: a part with a huge pile of other automatic styles, or one
            // oversized layout, costs that element and not the master pages after it.
            if !empty && read_page_layouts(&mut rd, &mut layouts).is_err() {
                return out;
            }
            continue;
        }
        if e.local_name().as_ref() != b"master-styles" {
            if !empty && skip_rest(&mut rd).is_err() {
                return out;
            }
            continue;
        }
        let mut budget = Budget::odf(500_000, 16 * 1024 * 1024);
        let Ok(Tree::Ok(node)) = read_element(&mut rd, &e, empty, &mut budget) else {
            return out;
        };
        for page in node.nodes().filter(|n| n.name == "master-page") {
            let Some(name) = page.attr("name") else {
                continue;
            };
            if out.frames.len() >= MAX_MASTERS {
                break;
            }
            let mut m = Master::new();
            for f in page.nodes() {
                if f.name != "frame" && !SHAPES.contains(&f.name.as_str()) {
                    continue;
                }
                if let (Some(class), Some(r)) = (f.attr("class"), own_rect(f)) {
                    m.entry(class.to_string()).or_insert(r);
                }
            }
            if let Some(r) = page.attr("page-layout-name").and_then(|l| layouts.get(l)) {
                out.size.insert(name.to_string(), *r);
            }
            out.frames.insert(name.to_string(), m);
        }
        return out;
    }
}

pub(in super::super) fn convert(
    path: &Path,
    opts: &DocOptions,
    cancel: Option<&Cancel>,
) -> Result<Document, OfficeError> {
    let cap = opts.limits.max_part_bytes;
    let mut pkg = Pkg::open(path)?;
    // The package must say it is an OpenDocument *presentation* (a text or a spreadsheet is not ours).
    let mime = match pkg.part("mimetype", 512)? {
        Some(mut r) => {
            let mut b = Vec::new();
            r.read_to_end(&mut b)
                .map_err(|e| OfficeError::Corrupt(format!("mimetype: {e}")))?;
            b
        }
        None => return Err(OfficeError::Unsupported),
    };
    if !mime
        .trim_ascii()
        .starts_with(b"application/vnd.oasis.opendocument.presentation")
    {
        return Err(OfficeError::Unsupported);
    }

    // A damaged styles part costs the document its styles, not the document.
    let mut st = OdStyles::default();
    if let Some(r) = pkg.part("styles.xml", cap)? {
        let _ = st.read(r);
    }
    let masters = match pkg.part("styles.xml", cap)? {
        Some(r) => read_masters(r),
        None => Masters::default(),
    };

    let media = Pkg::open(path)?;
    let conv = Conv::new(
        opts,
        cancel,
        Styles::default(),
        Numbering::default(),
        HashMap::new(),
        media,
    );
    let mut od = Od::new(conv, st);
    od.slides = true;
    od.c.split_bullet_lists = true;
    let mut slides: Vec<SlideInfo> = Vec::new();
    {
        let Some(r) = pkg.part("content.xml", cap)? else {
            return Err(OfficeError::Corrupt("missing content.xml".into()));
        };
        od.read_slides(r, &masters, &mut slides)?;
    }
    od.c.flush_carry_top();
    od.c.flush_code();
    let Od { c, defs, .. } = od;
    let mut doc = c.assemble(defs);
    doc.slides = slides;
    Ok(doc)
}

impl Od<'_> {
    /// Reads `content.xml`: the automatic styles, then the slides one by one.
    fn read_slides(
        &mut self,
        src: impl BufRead,
        masters: &Masters,
        slides: &mut Vec<SlideInfo>,
    ) -> Result<(), OfficeError> {
        let mut rd = XmlReader::new(src);
        let mut buf = Vec::new();
        if !self.enter_body(&mut rd, &mut buf, "presentation")? {
            return Ok(());
        }
        let opts = self.c.opts;
        let text_cap = opts
            .max_block_text_bytes
            .min(usize::try_from(opts.max_slide_part_bytes).unwrap_or(usize::MAX));
        let mut count = 0usize;
        loop {
            buf.clear();
            let (e, empty) = match rd.read_event_into(&mut buf).map_err(xml_err)? {
                Event::Start(e) => (e.into_owned(), false),
                Event::Empty(e) => (e.into_owned(), true),
                Event::End(_) => return Ok(()),
                Event::Eof => {
                    return Err(OfficeError::Corrupt(
                        "xml: unexpected end of document".into(),
                    ))
                }
                _ => continue,
            };
            if e.local_name().as_ref() != b"page" {
                if !empty {
                    skip_rest(&mut rd)?;
                }
                continue;
            }
            // (The whole presentation is one part: the budget of XML read over a deck, as the
            // PowerPoint reader has, ends the reading of the slides.)
            if count >= opts.max_slides
                || self.cancelled()
                || self.c.full
                || self.order_shapes > opts.max_deck_order_shapes
                || rd.position() > opts.max_pptx_read_total
            {
                self.c.truncated = true;
                return Ok(());
            }
            count += 1;
            let mut budget = Budget::odf(opts.max_block_nodes, text_cap);
            let info = match read_element(&mut rd, &e, empty, &mut budget)? {
                Tree::Ok(page) => self.slide(masters, count, &page),
                Tree::TooBig => {
                    // A slide that cannot be read still has its place in the order.
                    self.c.truncated = true;
                    write_heading(&mut self.c, count, "", false)
                }
            };
            if let Some(info) = info {
                slides.push(info);
            }
            if self.c.full {
                self.c.truncated = true;
                return Ok(());
            }
        }
    }

    /// Writes one slide; `None` when not even its heading fit the output budgets.
    fn slide(&mut self, masters: &Masters, number: usize, page: &Node) -> Option<SlideInfo> {
        let hidden = page
            .attr("style-name")
            .is_some_and(|s| self.st.page_hidden(s));
        let mut b = Build {
            master: page
                .attr("master-page-name")
                .and_then(|m| masters.frames.get(m)),
            shapes: 0,
            max_shapes: self.c.opts.max_slide_shapes,
            truncated: false,
        };
        self.slide_no = number;
        self.slide_rect = page
            .attr("master-page-name")
            .and_then(|m| masters.size.get(m))
            .copied();
        let mut items: Vec<Sh<'_>> = Vec::new();
        build(page.nodes(), &mut b, 0, &mut items);
        if b.truncated {
            self.c.truncated = true;
        }
        // The first title that has text is the heading; any other is text.
        let title = items.iter().enumerate().find_map(|(i, s)| {
            if s.kind != Kind::Title {
                return None;
            }
            let n = match &s.body {
                Body::Frame(n) | Body::Shape(n) => *n,
                Body::Group(_) => return None,
            };
            let t = plain_title(n);
            (!t.is_empty()).then_some((i, t))
        });
        let (title_idx, title) = match title {
            Some((i, t)) => (Some(i), t),
            None => (None, String::new()),
        };
        let info = write_heading(&mut self.c, number, &title, hidden)?;
        if let Some(i) = title_idx {
            // (A title kept as text must not claim the first place twice.)
            for (k, s) in items.iter_mut().enumerate() {
                if s.kind == Kind::Title && k != i {
                    s.kind = Kind::Subtitle;
                }
            }
        }
        self.write_items(&items, title_idx, 0);
        if let Some(notes) = page.child("notes") {
            self.write_notes(notes);
        }
        Some(info)
    }

    fn write_items(&mut self, items: &[Sh<'_>], skip: Option<usize>, depth: usize) {
        if depth > MAX_GROUP_DEPTH {
            return;
        }
        // The reading-order pass is paid for per shape over the whole presentation: past the
        // budget the rest of it is left out (the slide that crossed it keeps its heading).
        self.order_shapes = self.order_shapes.saturating_add(items.len());
        if self.order_shapes > self.c.opts.max_deck_order_shapes {
            self.c.truncated = true;
            return;
        }
        let shapes: Vec<Shape> = items
            .iter()
            .map(|s| Shape {
                rect: s.rect,
                kind: s.kind,
            })
            .collect();
        for i in reading_order(&shapes, self.slide_rect) {
            if Some(i) == skip {
                continue;
            }
            if self.cancelled() || self.c.full {
                self.c.truncated = true;
                return;
            }
            let mut blks = Vec::new();
            match &items[i].body {
                Body::Frame(f) => self.frame_blocks(f, &mut blks),
                Body::Shape(s) => self.block(s, Ctx::Body, 0, &mut blks),
                Body::Group(kids) => {
                    self.write_items(kids, None, depth + 1);
                    continue;
                }
            }
            self.c.write_blocks(blks);
        }
    }

    /// The blocks of a frame: a table (not the preview picture stored beside it), a chart, or what
    /// a Writer frame shows (text box, picture, formula).
    fn frame_blocks(&mut self, f: &Node, out: &mut Vec<Blk>) {
        self.c.cur_ctx = Ctx::Body;
        if let Some(t) = f.child("table") {
            self.block(t, Ctx::Body, 1, out);
            return;
        }
        for o in f.nodes().filter(|k| k.name == "object") {
            if let Some(title) = self.chart_title(o) {
                let label = match title {
                    Some(t) => format!("chart: {t}"),
                    None => "chart".to_string(),
                };
                self.c.cur_ctx = Ctx::Body;
                out.push(Blk::Para(self.c.placeholder(&label)));
                return;
            }
        }
        // What a Writer frame shows; a frame that is only a formula is a display formula (as a
        // paragraph that is only a formula is in Writer).
        let mut inl = Inl::default();
        let mut ps = Ps {
            prev_ws: true,
            ..Ps::default()
        };
        self.frame(f, Ctx::Body, &mut inl, &mut ps, 0);
        if ps.math.len() == 1 && !ps.other && inl.extras.is_empty() {
            if let Some((i, latex)) = ps.math.pop() {
                if i < inl.segs.len() {
                    inl.segs[i] = Seg::Display(Ok(latex));
                }
            }
        }
        self.flush_inline(inl, Ctx::Body, out);
    }

    /// Whether the embedded object `o` is a chart: `Some(its title)` if so. Only the start of its
    /// `content.xml` is read (the part is bounded and charged to the chart-read budget).
    fn chart_title(&mut self, o: &Node) -> Option<Option<String>> {
        let dir = part_of(o.attr("href")?)?;
        if dir.len() > OBJECT_DIR_MAX {
            return None;
        }
        let dir = dir.trim_end_matches('/');
        let left = CHART_READ_BUDGET.saturating_sub(self.c.chart_read);
        if left == 0 || self.cancelled() {
            return None;
        }
        let limit = left.min(CHART_PART_BYTES);
        let mut bytes = Vec::new();
        {
            let r = self
                .c
                .media
                .part(&format!("{dir}/content.xml"), limit)
                .ok()??;
            r.take(limit).read_to_end(&mut bytes).ok()?;
        }
        // (An object that is not a chart costs the budget as well: a deck of thousands of formulas
        // is not read thousands of times.)
        self.c.chart_read += (bytes.len() as u64).max(1024);
        let mut rd = XmlReader::new(&bytes[..]);
        let mut buf = Vec::new();
        let mut path: Vec<String> = Vec::new();
        let mut chart = false;
        loop {
            buf.clear();
            let (e, empty) = match rd.read_event_into(&mut buf) {
                Ok(Event::Start(e)) => (e.into_owned(), false),
                Ok(Event::Empty(e)) => (e.into_owned(), true),
                Ok(Event::End(_)) => {
                    path.pop();
                    continue;
                }
                Ok(Event::Eof) | Err(_) => return chart.then_some(None),
                _ => continue,
            };
            let name = String::from_utf8_lossy(e.local_name().as_ref()).into_owned();
            path.push(name);
            // A root that is not `office:document-content` (a formula is `math:math`) or a body
            // whose content is not a chart: not a chart.
            if (path.len() == 1 && path[0] != "document-content")
                || (path.len() == 3 && path[1] == "body" && path[2] != "chart")
            {
                return None;
            }
            let p: Vec<&str> = path.iter().map(String::as_str).collect();
            match p.as_slice() {
                ["document-content", "body", "chart"] => chart = true,
                // The chart's own title: not the title of an axis or a legend.
                ["document-content", "body", "chart", "chart", "title"] => {
                    let mut budget = Budget::odf(5_000, 16 * 1024);
                    let Ok(Tree::Ok(node)) = read_element(&mut rd, &e, empty, &mut budget) else {
                        return Some(None);
                    };
                    let mut text = String::new();
                    node_text(&node, &mut text, 0);
                    let text: String = clean(&text)
                        .split_whitespace()
                        .collect::<Vec<_>>()
                        .join(" ")
                        .chars()
                        .take(200)
                        .collect();
                    return Some((!text.is_empty()).then_some(text));
                }
                _ => {}
            }
            if empty {
                path.pop();
            }
        }
    }

    /// The speaker notes of a slide: the text of the frames of class `notes` of its notes page
    /// (not the picture of the slide), as a quote block.
    fn write_notes(&mut self, notes: &Node) {
        let mut lines: Vec<String> = Vec::new();
        let frames = notes
            .nodes()
            .filter(|f| f.name == "frame" && f.attr("class") == Some("notes"));
        for f in frames.take(MAX_NOTE_FRAMES) {
            for tb in f.nodes().filter(|k| k.name == "text-box") {
                let mut blks = Vec::new();
                self.blocks(&tb.kids, Ctx::Note, 0, &mut blks);
                lines.extend(blk_strings(&blks, "\n", true).into_iter().map(|(s, _)| s));
            }
        }
        self.c.cur_ctx = Ctx::Body;
        write_notes_quote(&mut self.c, lines);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(attrs: &[(&str, &str)]) -> Node {
        Node {
            prefix: "draw".into(),
            name: "frame".into(),
            attrs: attrs
                .iter()
                .map(|(k, v)| (format!("svg:{k}"), (*v).to_string()))
                .collect(),
            kids: Vec::new(),
        }
    }

    #[test]
    fn lengths_are_converted_to_emu() {
        for (v, want) in [
            ("1in", 914_400.0),
            ("2.54cm", 914_400.0),
            ("25.4mm", 914_400.0),
            ("72pt", 914_400.0),
            ("6pc", 914_400.0),
            ("96px", 914_400.0),
            (" -1cm ", -360_000.0),
            ("+0.5cm", 180_000.0),
            ("0cm", 0.0),
        ] {
            assert_eq!(emu(v), Some(want), "{v}");
        }
        for v in [
            "", "cm", "1", "1em", "50%", "1 cm cm", "abc", "1,5cm", "NaNcm", "1e3cm",
        ] {
            assert_eq!(emu(v), None, "{v}");
        }
        // Huge values are clamped, never infinite.
        let big = emu("99999999999999999999999cm").unwrap();
        assert!(big.is_finite() && big <= LIM);
    }

    #[test]
    fn a_frame_states_its_rectangle() {
        let n = node(&[
            ("x", "1in"),
            ("y", "2in"),
            ("width", "3in"),
            ("height", "4in"),
        ]);
        assert_eq!(
            own_rect(&n),
            Some(Rect::new(914_400, 1_828_800, 2_743_200, 3_657_600))
        );
        // All four are needed (no transform): a partial position is no position.
        for missing in ["x", "y", "width", "height"] {
            let attrs: Vec<(&str, &str)> = [
                ("x", "1cm"),
                ("y", "1cm"),
                ("width", "1cm"),
                ("height", "1cm"),
            ]
            .into_iter()
            .filter(|(k, _)| *k != missing)
            .collect();
            assert_eq!(own_rect(&node(&attrs)), None, "{missing}");
        }
        assert_eq!(own_rect(&node(&[])), None);
    }

    #[test]
    fn a_rotation_gives_the_bounding_box_of_the_corners() {
        // 4in x 2in, turned a quarter (counter-clockwise on a y-down page: the matrix is the SVG
        // one) about the origin and moved: the box is 2in x 4in.
        let n = node(&[
            ("width", "4in"),
            ("height", "2in"),
            ("transform", "rotate (1.5707963267949) translate (5in 1in)"),
        ]);
        let r = own_rect(&n).unwrap();
        assert_eq!((r.w, r.h), (1_828_800, 3_657_600));
        // translate first (5in, 1in), then rotate by 90deg: (x, y) -> (-y, x).
        assert!((r.x - (-3 * 914_400)).abs() < 3, "{r:?}");
        assert!((r.y - (5 * 914_400)).abs() < 3, "{r:?}");
        // Without x and y but with a transform the shape is placed by the transform alone.
        let t = node(&[
            ("width", "1in"),
            ("height", "1in"),
            ("transform", "translate (2in 3in)"),
        ]);
        assert_eq!(
            own_rect(&t),
            Some(Rect::new(1_828_800, 2_743_200, 914_400, 914_400))
        );
        // x and y are the unrotated position.
        let xy = node(&[
            ("x", "1in"),
            ("y", "1in"),
            ("width", "1in"),
            ("height", "1in"),
            ("transform", "translate (1in 0in)"),
        ]);
        assert_eq!(
            own_rect(&xy),
            Some(Rect::new(1_828_800, 914_400, 914_400, 914_400))
        );
    }

    #[test]
    fn transforms_compose_in_the_order_of_svg() {
        let m = transform_of("scale (2) translate (1cm 0cm)").unwrap();
        // translate first, then scale: x = (x + 1cm) * 2.
        assert_eq!((m[0], m[3], m[4], m[5]), (2.0, 2.0, 720_000.0, 0.0));
        assert_eq!(transform_of("").unwrap(), [1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
        let mx = transform_of("matrix (1 0 0 1 2cm 3cm)").unwrap();
        assert_eq!((mx[4], mx[5]), (720_000.0, 1_080_000.0));
        let sk = transform_of("skewX (0.7853981633974483)").unwrap();
        assert!((sk[2] - 1.0).abs() < 1e-9);
        for bad in [
            "rotate",
            "rotate (",
            "rotate (a)",
            "bogus (1)",
            "translate (1)",
            "scale (1 2 3)",
            "matrix (1 2)",
            "rotate (1e999)",
        ] {
            assert_eq!(transform_of(bad), None, "{bad}");
        }
        assert_eq!(transform_of(&"rotate (0) ".repeat(17)), None);
    }

    #[test]
    fn a_shape_whose_transform_is_broken_keeps_its_stated_position() {
        let n = node(&[
            ("x", "1cm"),
            ("y", "1cm"),
            ("width", "1cm"),
            ("height", "1cm"),
            ("transform", "rotate (zzz)"),
        ]);
        assert_eq!(
            own_rect(&n),
            Some(Rect::new(360_000, 360_000, 360_000, 360_000))
        );
    }
}
