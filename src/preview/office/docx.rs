//! Word preview (docx / docm / dotx / dotm): the reading side. A document is read by konoma itself
//! and converted to a Markdown string that konoma's own Markdown renderer draws (design:
//! `docs/FEATURE-OFFICE-PREVIEW.md` section 10).
//!
//! # What is converted
//!
//! | Word | Markdown |
//! |---|---|
//! | paragraph style with `outlineLvl` (or named `Heading n` / `見出し n`), direct `outlineLvl` | `#` .. `######` |
//! | bold / italic / strike (`w:b` `w:i` `w:strike` `w:dstrike`, character and paragraph styles, document defaults, `val=0`) | `**` `*` `~~` |
//! | superscript / subscript (`w:vertAlign`, inherited like bold) | `<sup>` / `<sub>` (konoma draws `x²`, `H₂O`) |
//! | `w:br` / `w:cr` | hard line break; a page / column break ends the paragraph |
//! | `w:tab` | four no-break spaces |
//! | bullets | a real `- ` list (nested by indentation) |
//! | decimal `%N.` numbering | a real `N. ` list, **with the number Word shows** |
//! | every other number format (letters, roman, kana, `①`, `第1条`, `(a)`, multi-level `1.1`) | a paragraph that starts with Word's own label text |
//! | `w:tbl` | GFM table (merged cells: value top-left, covered cells empty; a row that starts late (`w:gridBefore`) starts with empty cells; nested tables flattened) |
//! | pictures (`a:blip`, VML `v:imagedata`) | `![alt](office-img://<hash>/<name>)` + [`DocImage`] |
//! | footnotes / endnotes | `[^n]` + `[^n]: text` (the paragraphs of a note are lines of it) |
//! | symbol fonts (`w:sym`, runs in Symbol / Wingdings ..) | the Unicode character they draw (`☑`, `✓`, `α`); a code the tables do not know is `•` |
//! | legacy form fields (`FORMCHECKBOX`, `FORMDROPDOWN`, `FORMTEXT`) | `☒` / `☐`, the chosen entry, the default text |
//! | charts, SmartArt | `[chart: title]`, `[SmartArt]` (the file has no picture of them) |
//! | `w:altChunk` (embedded HTML / RTF) | `[embedded document: name]` |
//! | hyperlinks, `HYPERLINK` fields, bookmarks | `[text](url)` / `[text](#heading-slug)` |
//! | fields (`fldSimple`, `fldChar`) | the stored result text |
//! | text boxes / shapes | their paragraphs, after the paragraph holding them |
//! | `mc:AlternateContent` | the first `Choice` whose `Requires` namespaces the reader understands (text boxes, groups, formulas ..), else the `Fallback` (a picture of what it cannot show) |
//! | `w:sdt`, `w:smartTag`, `w:customXml` | their content |
//! | `w:ruby` | the base text, then its reading in brackets (`漢字（かんじ）`) |
//! | `m:oMath` / `m:oMathPara` | `$latex$` / `$$latex$$` via `omml::to_latex`; the formula's characters when it cannot convert |
//!
//! Tracked changes are shown as the **final** version (`w:ins` / `w:moveTo` kept; `w:del` /
//! `w:moveFrom` dropped). Comments, headers and footers are not shown. Hidden text (`w:vanish`, also from a
//! character style, a paragraph style or the document defaults) is not shown.
//!
//! # Why a list label is the document's own text (design decision)
//!
//! A Markdown ordered list numbers itself (`1.` `2.` ...): the renderer cannot show `(a)`, `iv)`,
//! `ア`, `①` or `第1条`, and a list that restarts after an interruption would continue the old
//! count. So only the case Markdown can express exactly (a decimal level written `%N.`) becomes a
//! real list, and with the number Word computed (a new list starting at 5 is written `5.`); every
//! other label is written out as text at the start of its paragraph, indented with no-break spaces,
//! consecutive items joined by hard line breaks. What the reader sees is what Word showed.
//!
//! # Safety
//!
//! The package goes through [`container`] first (really inflated sizes, entry count, encryption).
//! Every XML part is read through `XmlReader` (depth 256, no DTD expansion) and the body is converted
//! **one top-level block at a time** with a node budget per block ([`DocOptions`]), so memory does not
//! grow with the document. The Markdown has a byte and a line budget; images have count, size and
//! total budgets; going over any of them stops the conversion and sets [`Document::truncated`].
//! Nothing here panics on any input (a panic inside is caught and reported as `Corrupt`).

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufRead, BufReader, Read};
use std::path::Path;

use quick_xml::events::Event;
use zip::ZipArchive;

use super::container::{self, Limits};
use super::docx_styles::{direct_num, format_number, render_label, Fmt, Numbering, Styles, Vert};
use super::docx_symbols::{map_run_text, map_sym};
use super::docx_xml::{read_element, to_xml, Budget, Kid, Node, Tree};
use super::fmt_xlsx::{attr, resolve_target, xml_err, XmlReader};
use super::workbook::Cancel;
use super::{omml, OfficeError};

// The OpenDocument text reader shares this module's converter (writer, escaping, notes, pictures),
// so it is a child module: it sees the private items it builds on.
#[path = "odt.rs"]
pub mod odt;
// The PowerPoint reader does the same: slides become Markdown through this module's writer.
#[path = "pptx.rs"]
pub mod pptx;

/// Limits and options of [`load_document`].
#[derive(Debug, Clone)]
pub struct DocOptions {
    /// Package limits (file size, entries, inflated sizes), shared with the spreadsheet readers.
    pub limits: Limits,
    /// Longest Markdown produced, in bytes. Under konoma's text preview cap (`text::MAX_BYTES`,
    /// 1 MiB) so the renderer never cuts the end of the document (the footnotes) off.
    pub max_markdown_bytes: usize,
    /// Most Markdown lines. konoma's Markdown view shows at most 5,000 lines.
    pub max_markdown_lines: usize,
    /// Most pictures kept. A report with a few hundred figures is large already.
    pub max_images: usize,
    /// Largest single picture kept (bytes).
    pub max_image_bytes: u64,
    /// Total bytes of pictures kept.
    pub max_total_image_bytes: u64,
    /// Most top-level blocks (paragraphs, tables) read. Empty paragraphs cost nothing in the output,
    /// so the output budget alone would not stop a body of millions of them.
    pub max_blocks: usize,
    /// Most elements one top-level block may hold (a block over it is skipped and the conversion
    /// stops). Bounds the memory of the tree held while one block is converted.
    pub max_block_nodes: usize,
    /// Most text bytes one top-level block may hold.
    pub max_block_text_bytes: usize,
    /// Most cells one table may have.
    pub max_table_cells: usize,
    /// Most distinct footnotes / endnotes shown.
    pub max_notes: usize,
    /// Longest raw XML of one formula handed to the math converter.
    pub max_math_xml: usize,
    /// The OMML -> LaTeX converter (`omml::to_latex`; a field so tests can stand in for it).
    pub math: fn(&str, bool) -> Option<String>,
    /// The MathML -> LaTeX converter of OpenDocument formulas (`mathml::to_latex`).
    pub mathml: fn(&str, bool) -> Option<String>,
    /// Most formula objects an OpenDocument text may have converted (each one is a zip part).
    pub max_math_objects: usize,
    /// The language of the words konoma writes into a converted presentation (the slide
    /// heading, the notes label).
    pub lang: crate::i18n::Lang,
    /// Most slides of a presentation read. Real decks have tens to a few hundred; every slide is
    /// at least a heading line and the Markdown view shows 5,000 lines, so more than this could
    /// never be shown anyway.
    pub max_slides: usize,
    /// Most shapes read from one slide (groups count their members). A slide has tens.
    pub max_slide_shapes: usize,
    /// Largest XML part of a presentation (slide, layout, master, notes, SmartArt) read at all
    /// (bytes of the inflated part). A slide of a few MB is a table of thousands of cells.
    pub max_slide_part_bytes: u64,
    /// Total bytes of such XML parts read over a whole presentation, which bounds the time spent
    /// parsing one. Measured on a release build (no LTO) on a machine with a load average of
    /// about 13, with `max_deck_order_shapes` still in force: at 64 MiB, 1,000 slides alternating
    /// between two slides of 5,000 empty `sp` each load in 0.68 s, 512 nested empty `sp` per slide
    /// in 0.59 s, and 200 distinct slides repeated 5 times in 0.62 s (32 MiB: 0.3-0.44 s; 128 MiB:
    /// 1.0-1.4 s). At a load average of 41 it takes about twice as long. 64 MiB because a deck of
    /// 1,000 slides with 50 text boxes each (about 17 MB of XML) uses only a quarter of it, which
    /// leaves room for heavy decks full of tables and diagrams to be read whole, at a worst case
    /// of under a second on this machine. Pictures are not XML parts and do not count.
    pub max_pptx_read_total: u64,
    /// Most shapes, over a whole presentation, handed to the reading-order pass (one call costs up
    /// to 3.4 microseconds a shape; the pass gives up above `UNDERLAY_MAX_SHAPES` a call, so this is
    /// at most about a third of a second). 1,000 slides of 50 shapes is half of it.
    pub max_deck_order_shapes: usize,
}

impl Default for DocOptions {
    fn default() -> Self {
        const MIB: u64 = 1024 * 1024;
        DocOptions {
            limits: Limits::default(),
            max_markdown_bytes: 1_000_000,
            max_markdown_lines: 5_000,
            max_images: 300,
            max_image_bytes: 16 * MIB,
            max_total_image_bytes: 96 * MIB,
            max_blocks: 1_000_000,
            max_block_nodes: 200_000,
            max_block_text_bytes: 8 * 1024 * 1024,
            max_table_cells: 20_000,
            max_notes: 5_000,
            max_math_xml: 64 * 1024,
            math: omml::to_latex,
            mathml: super::mathml::to_latex,
            max_math_objects: 5_000,
            lang: crate::i18n::Lang::En,
            max_slides: 1_000,
            max_slide_shapes: 5_000,
            max_slide_part_bytes: 16 * MIB,
            max_pptx_read_total: 64 * MIB,
            max_deck_order_shapes: 100_000,
        }
    }
}

/// A converted document.
#[derive(Debug, Clone, Default)]
pub struct Document {
    /// The document as Markdown (GFM + `$` math + `[^n]` footnotes).
    pub markdown: String,
    /// The pictures the Markdown refers to by [`DocImage::key`], in order of first use.
    pub images: Vec<DocImage>,
    /// The conversion stopped at a budget: the end of the document, some pictures, or part of a
    /// table is missing.
    pub truncated: bool,
    /// Distinct footnotes and endnotes shown (read by tests).
    #[cfg_attr(not(test), allow(dead_code))]
    pub notes: usize,
    /// Formulas found, and how many became LaTeX (the rest are shown as plain characters).
    #[cfg_attr(not(test), allow(dead_code))]
    pub math_total: usize,
    /// See [`Document::math_total`].
    #[cfg_attr(not(test), allow(dead_code))]
    pub math_latex: usize,
    /// A presentation: its slides in order (empty for other documents). There is exactly one
    /// level-2 heading in [`Document::markdown`] per entry, in the same order (and none besides).
    pub slides: Vec<pptx::SlideInfo>,
    /// One drawing model per entry of [`Document::slides`], same order; empty for Word documents
    /// and until the readers fill it.
    #[allow(dead_code)] // read by the app once the slide readers fill it
    pub slide_scenes: Vec<super::slide_draw::SlideScene>,
    /// A presentation's default view: for each slide, the same level-2 heading line as in
    /// `markdown` (identical text, same order, exactly one per slide and none besides), then a
    /// blank line, the slide picture as `![<alt>](<slide_keys[i]>)`, a blank line, and the slide's
    /// notes exactly as `markdown` writes them. Empty for Word documents and until a reader fills
    /// it (the app then shows `markdown`).
    #[allow(dead_code)] // read by the app once the slide readers fill it
    pub picture_markdown: String,
    /// The picture URL of each slide (`office-img://<12 hex>/slide-<n>.svg`, unique per conversion
    /// so two decks never share a cached picture), one per entry of `slide_scenes`, same order.
    #[allow(dead_code)] // read by the app once the slide readers fill it
    pub slide_keys: Vec<String>,
}

/// A picture of the document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocImage {
    /// The URL the Markdown uses (`office-img://<12 hex digits>/<name>`); the same bytes always
    /// give the same key.
    pub key: String,
    /// The file's bytes (png, jpeg, gif, bmp, webp, tiff or svg).
    pub bytes: Vec<u8>,
    /// The file name inside the package (`image1.png`).
    pub name: String,
}

/// Loads and converts a Word document.
#[cfg_attr(not(test), allow(dead_code))]
pub fn load_document(path: &Path, opts: &DocOptions) -> Result<Document, OfficeError> {
    load_document_cancellable(path, opts, None)
}

/// [`load_document`] that stops (with [`Document::truncated`] set) once `cancel` says so.
pub fn load_document_cancellable(
    path: &Path,
    opts: &DocOptions,
    cancel: Option<&Cancel>,
) -> Result<Document, OfficeError> {
    let names = container::inspect_word_package(path, &opts.limits)?;
    // The content decides, not the extension: an OpenDocument package carries `mimetype` first.
    // (A password-protected `.odt` has no readable `content.xml`; it is told apart here.)
    let odf = names.iter().any(|n| n == "mimetype");
    if odf && container::odf_encrypted(path, &names, &opts.limits) {
        return Err(OfficeError::Encrypted);
    }
    drop(names);
    let r = crate::preview::markdown::catch_silent(|| {
        if odf {
            odt::convert(path, opts, cancel)
        } else {
            convert(path, opts, cancel)
        }
    });
    match r {
        Some(r) => r,
        None => Err(OfficeError::Corrupt(
            "panic while reading the document".into(),
        )),
    }
}

// ---------------------------------------------------------------------------------------------
// package
// ---------------------------------------------------------------------------------------------

fn norm_name(n: &str) -> String {
    n.replace('\\', "/").to_ascii_lowercase()
}

struct Pkg {
    zip: ZipArchive<BufReader<File>>,
    names: HashMap<String, String>,
}

impl Pkg {
    fn open(path: &Path) -> Result<Pkg, OfficeError> {
        let zip = container::open_zip(path)?;
        let names = zip
            .file_names()
            .map(|n| (norm_name(n), n.to_string()))
            .collect();
        Ok(Pkg { zip, names })
    }

    fn has(&self, part: &str) -> bool {
        self.names.contains_key(&norm_name(part))
    }

    fn part<'z>(
        &'z mut self,
        part: &str,
        cap: u64,
    ) -> Result<Option<BufReader<impl Read + 'z>>, OfficeError> {
        let name = self
            .names
            .get(&norm_name(part))
            .cloned()
            .unwrap_or_else(|| part.to_string());
        Ok(container::part_reader(&mut self.zip, &name, cap)?
            .map(|r| BufReader::with_capacity(64 * 1024, r)))
    }
}

#[derive(Debug, Clone)]
struct Rel {
    kind: String,
    /// The part (internal, no leading slash) or the URL (external).
    target: String,
    external: bool,
}

fn parse_rels(src: impl BufRead, base: &str) -> Result<HashMap<String, Rel>, OfficeError> {
    let mut rd = XmlReader::new(src);
    let mut buf = Vec::new();
    let mut map = HashMap::new();
    loop {
        buf.clear();
        match rd.read_event_into(&mut buf).map_err(xml_err)? {
            Event::Start(e) | Event::Empty(e) if e.local_name().as_ref() == b"Relationship" => {
                let (Some(id), Some(target)) = (attr(&e, b"Id", false), attr(&e, b"Target", false))
                else {
                    continue;
                };
                let external = attr(&e, b"TargetMode", false)
                    .is_some_and(|m| m.eq_ignore_ascii_case("External"));
                let kind = attr(&e, b"Type", false)
                    .and_then(|t| t.rsplit('/').next().map(str::to_string))
                    .unwrap_or_default();
                let target = if external {
                    target
                } else {
                    resolve_target(base, &target)
                };
                if map.len() < 50_000 {
                    map.insert(
                        id,
                        Rel {
                            kind,
                            target,
                            external,
                        },
                    );
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(map)
}

/// `word/document.xml` -> (`word`, `word/_rels/document.xml.rels`).
fn rels_path(part: &str) -> (String, String) {
    match part.rsplit_once('/') {
        Some((dir, file)) => (dir.to_string(), format!("{dir}/_rels/{file}.rels")),
        None => (String::new(), format!("_rels/{part}.rels")),
    }
}

fn read_rels_of(pkg: &mut Pkg, part: &str, cap: u64) -> Result<HashMap<String, Rel>, OfficeError> {
    let (dir, rp) = rels_path(part);
    match pkg.part(&rp, cap)? {
        Some(r) => parse_rels(r, &dir),
        None => Ok(HashMap::new()),
    }
}

fn find_part(rels: &HashMap<String, Rel>, kind: &str, default: &str, pkg: &Pkg) -> Option<String> {
    let mut found: Vec<&Rel> = rels
        .values()
        .filter(|r| r.kind == kind && !r.external)
        .collect();
    found.sort_by(|a, b| a.target.cmp(&b.target));
    if let Some(r) = found.first() {
        if pkg.has(&r.target) {
            return Some(r.target.clone());
        }
    }
    pkg.has(default).then(|| default.to_string())
}

// ---------------------------------------------------------------------------------------------
// conversion state
// ---------------------------------------------------------------------------------------------

/// Concrete character formatting of a piece of text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct F {
    b: bool,
    i: bool,
    s: bool,
    /// Superscript / subscript: written as `<sup>` / `<sub>`, which konoma draws as Unicode
    /// super- / subscript characters when it can.
    v: Vert,
}

#[derive(Debug, Clone)]
enum Seg {
    Text(String, F),
    /// Already Markdown (an image, a footnote mark).
    Raw(String),
    /// Inline math: its LaTeX.
    Math(String),
    Break,
    PageBreak,
    LinkOpen(String),
    LinkClose,
    /// A display formula: its LaTeX, or its characters.
    Display(Result<String, String>),
}

#[derive(Debug, Default)]
struct Inl {
    segs: Vec<Seg>,
    /// Text boxes found in the paragraph: placed after it.
    extras: Vec<Blk>,
    /// Bookmark names started in the paragraph.
    bookmarks: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Marker {
    Bullet,
    Ordered(u32),
    Literal,
}

#[derive(Debug, Clone)]
struct Item {
    level: u8,
    list: u32,
    marker: Marker,
    label: String,
    text: String,
}

#[derive(Debug, Clone)]
enum Blk {
    Heading {
        level: u8,
        text: String,
        plain: String,
        bookmarks: Vec<String>,
    },
    Para(String),
    Item(Item),
    Code(String),
    Table(Vec<Vec<String>>),
    Math(String),
}

/// Where a block's text is going. A table cell is drawn by konoma's own flat scanner (no backslash
/// escapes, no nesting) and a note body by the full inline parser; the body too.
#[derive(PartialEq, Eq, Clone, Copy, Debug)]
enum Ctx {
    Body,
    Cell,
    Note,
}

impl Ctx {
    /// Blocks are flattened to one inline string (no headings, lists or nested tables).
    fn flat(self) -> bool {
        self != Ctx::Body
    }
}

#[derive(PartialEq, Eq, Clone, Copy)]
enum Phase {
    Instr,
    Result,
}

struct Field {
    instr: String,
    phase: Phase,
    link_open: bool,
    /// A legacy form field (`w:ffData` of the `begin` mark): what Word shows when the field holds
    /// no result text of its own.
    form: Option<String>,
    /// `inl.segs.len()` when the result began (a result that added nothing leaves the field empty).
    result_from: usize,
    /// The result already held something in an earlier paragraph (a field can span paragraphs, and
    /// `result_from` is a position in the paragraph being read only).
    has_result: bool,
    /// The run that began the field is hidden (`w:vanish`): so is its form data.
    hidden: bool,
}

/// What a legacy form field (`FORMCHECKBOX`, `FORMDROPDOWN`, `FORMTEXT`) shows, from the
/// `w:ffData` of its `begin` mark.
fn form_field_text(ff: &Node) -> Option<String> {
    if let Some(cb) = ff.child("checkBox") {
        // The state: `w:checked` when present (a bare element is true), else `w:default`.
        let on = cb
            .toggle("checked")
            .or_else(|| {
                cb.child("default")
                    .map(|_| cb.toggle("default").unwrap_or(false))
            })
            .unwrap_or(false);
        return Some(if on { "\u{2612}" } else { "\u{2610}" }.to_string());
    }
    if let Some(dd) = ff.child("ddList") {
        let pick = |name: &str| {
            dd.child(name)
                .and_then(|n| n.attr("val"))
                .and_then(|v| v.trim().parse::<usize>().ok())
        };
        let idx = pick("result").or_else(|| pick("default")).unwrap_or(0);
        return dd
            .nodes()
            .filter(|n| n.name == "listEntry")
            .nth(idx)
            .and_then(|n| n.attr("val"))
            .map(clean)
            .filter(|t| !t.trim().is_empty());
    }
    if let Some(ti) = ff.child("textInput") {
        return ti
            .child("default")
            .and_then(|n| n.attr("val"))
            .map(clean)
            .filter(|t| !t.trim().is_empty());
    }
    None
}

/// What a paragraph is.
struct Role {
    heading: Option<u8>,
    code: bool,
    list: Option<ListRole>,
}

struct ListRole {
    level: u8,
    list: u32,
    marker: Marker,
    label: String,
}

#[derive(PartialEq, Eq, Clone, Copy)]
enum Last {
    None,
    Other,
    Real,
    Literal,
}

struct Conv<'a> {
    opts: &'a DocOptions,
    cancel: Option<&'a Cancel>,
    styles: Styles,
    numbering: Numbering,
    counters: HashMap<u32, [Option<i64>; 9]>,
    seen_nums: HashSet<u32>,
    rels: HashMap<String, Rel>,
    media: Pkg,
    images: Vec<DocImage>,
    image_by_part: HashMap<String, Option<String>>,
    image_total: u64,
    /// Chart titles by part: a chart drawn many times is read once.
    chart_titles: HashMap<String, Option<String>>,
    /// Bytes of chart parts read so far (see `CHART_READ_BUDGET`).
    chart_read: u64,
    note_labels: Vec<(bool, i64)>,
    note_index: HashMap<(bool, i64), usize>,
    in_note: bool,
    cur_ctx: Ctx,
    bookmark_slugs: HashMap<String, String>,
    slug_counts: HashMap<String, usize>,
    anchor_names: Vec<String>,
    anchor_index: HashMap<String, usize>,
    fields: Vec<Field>,
    carry: Vec<Seg>,
    /// How much of `carry` is already known to hold something besides spaces: `(length scanned,
    /// it does)`. A deletion that spans many paragraphs would otherwise rescan the whole carry
    /// at each one.
    carry_seen: (usize, bool),
    truncated: bool,
    math_total: usize,
    math_latex: usize,
    // writer
    out: String,
    out_lines: usize,
    last: Last,
    last_list: Option<(u32, bool)>,
    /// Lists of the same kind with different ids that follow one another are separate lists, not
    /// one (slides: the bullets of one shape and the bullets of the next).
    split_bullet_lists: bool,
    /// The list id of the last item written with a literal label (`a.` `(i)` ..).
    last_literal: Option<u32>,
    stack: Vec<(u8, usize)>,
    pending_code: Option<String>,
    /// Bytes of the note definitions held until the end of the document (they are written after
    /// the body, so the body's own budget does not see them).
    defs_bytes: usize,
    body_bytes: usize,
    body_lines: usize,
    full: bool,
}

impl<'a> Conv<'a> {
    /// A converter with empty output; the reader fills it block by block.
    fn new(
        opts: &'a DocOptions,
        cancel: Option<&'a Cancel>,
        styles: Styles,
        numbering: Numbering,
        rels: HashMap<String, Rel>,
        media: Pkg,
    ) -> Conv<'a> {
        Conv {
            opts,
            cancel,
            styles,
            numbering,
            counters: HashMap::new(),
            seen_nums: HashSet::new(),
            rels,
            media,
            images: Vec::new(),
            image_by_part: HashMap::new(),
            image_total: 0,
            chart_titles: HashMap::new(),
            chart_read: 0,
            note_labels: Vec::new(),
            note_index: HashMap::new(),
            in_note: false,
            cur_ctx: Ctx::Body,
            bookmark_slugs: HashMap::new(),
            slug_counts: HashMap::new(),
            anchor_names: Vec::new(),
            anchor_index: HashMap::new(),
            fields: Vec::new(),
            carry: Vec::new(),
            carry_seen: (0, false),
            truncated: false,
            math_total: 0,
            math_latex: 0,
            out: String::new(),
            out_lines: 0,
            last: Last::None,
            last_list: None,
            split_bullet_lists: false,
            last_literal: None,
            stack: Vec::new(),
            pending_code: None,
            defs_bytes: 0,
            body_bytes: opts
                .max_markdown_bytes
                .saturating_sub((opts.max_markdown_bytes / 8).min(256 * 1024)),
            body_lines: opts
                .max_markdown_lines
                .saturating_sub((opts.max_markdown_lines / 10).min(500)),
            full: false,
        }
    }
}

const TOKEN_OPEN: char = '\u{E000}';
const TOKEN_CLOSE: char = '\u{E001}';
const NBSP: char = '\u{00A0}';

fn convert(
    path: &Path,
    opts: &DocOptions,
    cancel: Option<&Cancel>,
) -> Result<Document, OfficeError> {
    let cap = opts.limits.max_part_bytes;
    let mut pkg = Pkg::open(path)?;

    // The main part: named by the package relationships, else the usual name.
    let root_rels = read_rels_of_root(&mut pkg, cap).unwrap_or_default();
    let main = root_rels
        .values()
        .find(|r| r.kind == "officeDocument" && !r.external)
        .map(|r| r.target.clone())
        .filter(|t| pkg.has(t))
        .or_else(|| {
            pkg.has("word/document.xml")
                .then(|| "word/document.xml".to_string())
        })
        .ok_or(OfficeError::Unsupported)?;
    let rels = read_rels_of(&mut pkg, &main, cap).unwrap_or_default();

    // A damaged styles / numbering part costs the document its styles, not the document.
    let styles = match find_part(&rels, "styles", "word/styles.xml", &pkg) {
        Some(p) => match pkg.part(&p, cap)? {
            Some(r) => Styles::parse(r).unwrap_or_default(),
            None => Styles::default(),
        },
        None => Styles::default(),
    };
    let numbering = match find_part(&rels, "numbering", "word/numbering.xml", &pkg) {
        Some(p) => match pkg.part(&p, cap)? {
            Some(r) => Numbering::parse(r).unwrap_or_default(),
            None => Numbering::default(),
        },
        None => Numbering::default(),
    };
    let footnotes_part = find_part(&rels, "footnotes", "word/footnotes.xml", &pkg);
    let endnotes_part = find_part(&rels, "endnotes", "word/endnotes.xml", &pkg);

    let media = Pkg::open(path)?;
    let mut conv = Conv::new(opts, cancel, styles, numbering, rels, media);

    {
        let Some(r) = pkg.part(&main, cap)? else {
            return Err(OfficeError::Unsupported);
        };
        conv.read_body(r)?;
    }
    conv.flush_carry_top();
    conv.flush_code();

    // Footnotes and endnotes, only the referenced ones.
    let mut defs: Vec<(usize, String)> = Vec::new();
    if !conv.note_labels.is_empty() {
        for (is_end, part) in [(false, footnotes_part), (true, endnotes_part)] {
            let Some(part) = part else { continue };
            let note_rels = read_rels_of(&mut pkg, &part, cap).unwrap_or_default();
            let Some(r) = pkg.part(&part, cap)? else {
                continue;
            };
            // A damaged notes part leaves the references without text, not without a document.
            if conv.read_notes(r, note_rels, is_end, &mut defs).is_err() {
                conv.truncated = true;
            }
        }
    }
    Ok(conv.assemble(defs))
}

impl Conv<'_> {
    /// The finished document: the body, then the notes (`[^n]: text`, in order of reference) when
    /// they fit the budgets, with the heading anchors resolved.
    fn assemble(self, mut defs: Vec<(usize, String)>) -> Document {
        let mut conv = self;
        let opts = conv.opts;
        let mut md = std::mem::take(&mut conv.out);
        defs.sort_by_key(|d| d.0);
        let mut notes = 0usize;
        for (n, text) in &defs {
            let line = format!("[^{n}]: {text}");
            if md.len() + line.len() + 2 > opts.max_markdown_bytes
                || md.matches('\n').count() + line.matches('\n').count() + 2
                    > opts.max_markdown_lines
            {
                conv.truncated = true;
                break;
            }
            if !md.is_empty() {
                md.push_str(if notes == 0 { "\n\n" } else { "\n" });
            }
            md.push_str(&line);
            notes += 1;
        }
        let md = conv.resolve_anchors(md);
        Document {
            markdown: md,
            images: conv.images,
            truncated: conv.truncated,
            notes,
            math_total: conv.math_total,
            math_latex: conv.math_latex,
            slides: Vec::new(),
            slide_scenes: Vec::new(),
            picture_markdown: String::new(),
            slide_keys: Vec::new(),
        }
    }
}

fn read_rels_of_root(pkg: &mut Pkg, cap: u64) -> Result<HashMap<String, Rel>, OfficeError> {
    match pkg.part("_rels/.rels", cap)? {
        Some(r) => parse_rels(r, ""),
        None => Ok(HashMap::new()),
    }
}

// ---------------------------------------------------------------------------------------------
// text helpers
// ---------------------------------------------------------------------------------------------

/// Text from the document made safe for the Markdown: control characters dropped, newlines to
/// spaces, the private-use code points this module uses as tokens replaced.
fn clean(s: &str) -> String {
    s.chars()
        .filter_map(|c| match c {
            '\n' | '\r' => Some(' '),
            '\t' => Some(' '),
            // Unicode line separators (NEL, LS, PS): a line end for some readers, never ours.
            '\u{85}' | '\u{2028}' | '\u{2029}' => Some(' '),
            '\u{E000}'..='\u{E002}' => Some('\u{FFFD}'),
            c if (c as u32) < 0x20 || c == '\u{7F}' || c == '\u{FFFE}' || c == '\u{FFFF}' => None,
            // zero-width and bidi controls Word writes around some text
            '\u{200B}' | '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}' => None,
            c => Some(c),
        })
        .collect()
}

/// Whether `c` is written full-width (kanji, kana, hangul, full-width forms): what takes full-width
/// brackets.
fn is_cjk(c: char) -> bool {
    matches!(c as u32,
        0x2E80..=0x9FFF | 0xAC00..=0xD7AF | 0xF900..=0xFAFF | 0xFF00..=0xFFEF | 0x20000..=0x2FFFF)
}

fn is_punct(c: char) -> bool {
    c.is_ascii_punctuation()
        || matches!(c,
            '\u{00A1}'..='\u{00BF}'
            | '\u{2010}'..='\u{2027}'
            | '\u{2030}'..='\u{205E}'
            | '\u{3001}'..='\u{3003}'
            | '\u{3008}'..='\u{3011}'
            | '\u{3014}'..='\u{301F}'
            | '\u{FF01}'..='\u{FF0F}'
            | '\u{FF1A}'..='\u{FF20}'
            | '\u{FF3B}'..='\u{FF40}'
            | '\u{FF5B}'..='\u{FF65}')
}

/// Whether a `&...;` that starts at `rest[0] == '&'` could be taken for a character reference.
fn entity_like(rest: &[char]) -> bool {
    for (n, &c) in rest[1..].iter().enumerate() {
        if c == ';' {
            return n > 0;
        }
        if !(c.is_ascii_alphanumeric() || c == '#') || n > 31 {
            return false;
        }
    }
    false
}

/// Whether `:shortcode:` starts at `rest[0] == ':'`.
fn emoji_like(rest: &[char]) -> bool {
    for (n, &c) in rest[1..].iter().enumerate() {
        if c == ':' {
            return n > 0;
        }
        if !(c.is_ascii_alphanumeric() || c == '_' || c == '+' || c == '-') || n > 40 {
            return false;
        }
    }
    false
}

/// Escapes `s` so konoma's Markdown renderer shows it literally. `line_start`: `s` begins a
/// line (a block marker would be recognised there).
fn escape(s: &str, line_start: bool) -> String {
    let ch: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len() + 8);
    let mut digits_at_start = line_start;
    for (i, &c) in ch.iter().enumerate() {
        let at_start = line_start && i == 0;
        if !c.is_ascii_digit() {
            let was = digits_at_start;
            digits_at_start = false;
            let next_ok = ch.get(i + 1).is_none_or(|n| n.is_whitespace());
            if was && (c == '.' || c == ')') && i > 0 && i <= 9 && next_ok {
                out.push('\\');
                out.push(c);
                continue;
            }
        }
        match c {
            // `]` stays plain: a backslash before it could pair with an escaped `[` into the
            // renderer's `\[ ... \]` display-math delimiters.
            '\\' | '`' | '*' | '[' | '<' | '|' | '$' | '~' | '^' => {
                out.push('\\');
                out.push(c);
            }
            '_' => {
                let prev = i.checked_sub(1).and_then(|p| ch.get(p)).copied();
                let next = ch.get(i + 1).copied();
                let intra = prev.is_some_and(char::is_alphanumeric)
                    && next.is_some_and(char::is_alphanumeric);
                if !intra {
                    out.push('\\');
                }
                out.push('_');
            }
            '&' if entity_like(&ch[i..]) => out.push_str("\\&"),
            ':' if emoji_like(&ch[i..]) => out.push_str("\\:"),
            '>' | '#' | '-' | '+' | '=' if at_start => {
                out.push('\\');
                out.push(c);
            }
            c => out.push(c),
        }
        if !c.is_ascii_digit() {
            digits_at_start = false;
        }
    }
    out
}

/// Which markup characters of a table cell's text konoma's cell scanner would take for markup
/// (decided per paragraph from what the paragraph holds).
#[derive(Clone, Copy, Default)]
struct CellEsc {
    star: bool,
    grave: bool,
    tilde: bool,
}

impl CellEsc {
    fn of(segs: &[Seg]) -> CellEsc {
        let (mut stars, mut graves, mut tildes, mut emph) = (0usize, 0usize, 0usize, false);
        for g in segs {
            if let Seg::Text(t, f) = g {
                stars += t.matches('*').count();
                graves += t.matches('`').count();
                tildes += t.matches("~~").count();
                emph |= !marks_of(*f, true).is_empty();
            }
        }
        CellEsc {
            star: stars >= 2 || (stars >= 1 && emph),
            grave: graves >= 2,
            tilde: tildes >= 2 || (tildes >= 1 && emph),
        }
    }

    fn of_str(s: &str) -> CellEsc {
        CellEsc::of(&[Seg::Text(s.to_string(), F::default())])
    }
}

/// Text made literal for its place: Markdown backslash escapes for the body and note bodies; for a
/// table cell (whose scanner knows no backslash escapes, only `\|`) the characters that would be
/// taken for markup are replaced by look-alikes or split with a zero-width space.
fn esc_text(s: &str, line_start: bool, cell: bool, ce: CellEsc) -> String {
    if !cell {
        return escape(s, line_start);
    }
    let ch: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len() + 4);
    for (i, &c) in ch.iter().enumerate() {
        let next = ch.get(i + 1).copied();
        match c {
            '*' if ce.star => out.push('\u{2217}'),
            '`' if ce.grave => out.push('\u{02CB}'),
            '~' if ce.tilde => out.push('\u{223C}'),
            '|' => out.push_str("\\|"),
            '<' if next
                .is_some_and(|n| n.is_ascii_alphabetic() || matches!(n, '!' | '/' | '?')) =>
            {
                out.push('<');
                out.push('\u{200B}');
            }
            ']' if next == Some('(') => {
                out.push(']');
                out.push('\u{200B}');
            }
            '[' if next == Some('^') => {
                out.push('[');
                out.push('\u{200B}');
            }
            c => out.push(c),
        }
    }
    out
}

/// Brackets inside a link label would end it early: an entity for the body (the renderer decodes it;
/// a backslash escape could pair with another into the `\[ ... \]` math delimiters), a look-alike
/// for a table cell (whose scanner cuts a label at the first `]`).
fn label_fix(escaped: &str, cell: bool) -> String {
    if cell {
        escaped.replace('[', "\u{FF3B}").replace(']', "\u{FF3D}")
    } else {
        escaped.replace("\\[", "&#91;").replace(']', "&#93;")
    }
}

/// A display formula is written on a line of its own between two `$$` lines, where a first
/// character that starts a Markdown block (`> quote`, `---` rule, a code fence, a list marker, a
/// heading, a table row, HTML, a link definition) would make the formula that block. An empty
/// group in front is invisible in LaTeX and keeps it a formula.
fn guard_display(latex: &str) -> String {
    let mut cs = latex.chars();
    let digits = latex.chars().take_while(char::is_ascii_digit).count();
    let block_start = match cs.next() {
        Some('>' | '#' | '-' | '+' | '*' | '_' | '=' | '`' | '~' | '|' | '<' | ':' | '[' | '!') => {
            true
        }
        // `1.` / `1)` starts a numbered list.
        Some(c) if c.is_ascii_digit() => {
            let mut rest = latex[digits..].chars();
            matches!(rest.next(), Some('.' | ')')) && rest.next().is_none_or(char::is_whitespace)
        }
        _ => false,
    };
    if block_start {
        format!("{{}}{latex}")
    } else {
        latex.to_string()
    }
}

/// A line that is only `---` / `***` / `___` would be drawn as a rule whatever its escapes; a
/// zero-width space in front keeps it text. Applied to **every line** of a paragraph (the first and
/// each one after a hard line break): all of them begin a line for the Markdown reader, and the
/// other block markers (`#`, `>`, `-`, `+`, `1.`, `=`) are escaped at every line start by
/// [`emit`] already.
fn guard_rule(text: String) -> String {
    if !text.contains('\n') {
        return guard_rule_line(text);
    }
    text.split('\n')
        .map(|l| guard_rule_line(l.to_string()))
        .collect::<Vec<_>>()
        .join("\n")
}

fn guard_rule_line(text: String) -> String {
    // The renderer strips `<sup>`/`<sub>` before it reads blocks.
    let bare = text
        .replace("<sup>", "")
        .replace("</sup>", "")
        .replace("<sub>", "")
        .replace("</sub>", "");
    let core: String = bare
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '\\')
        .collect();
    let mut cs = core.chars();
    let rule = match cs.next() {
        Some(f @ ('-' | '*' | '_')) => core.chars().count() >= 3 && core.chars().all(|c| c == f),
        _ => false,
    };
    if rule {
        format!("\u{200B}{text}")
    } else {
        text
    }
}

/// A heading's text with a closing `#` sequence made literal: `Chapter #` would be read as the
/// heading `Chapter` with an ATX closing sequence (a run of `#` after a space at the end).
fn guard_atx_close(text: &str) -> String {
    let body = text.trim_end_matches('#');
    let hashes = text.len() - body.len();
    if hashes == 0 || body.ends_with('\\') {
        return text.to_string();
    }
    if body.is_empty() || body.ends_with([' ', '\t']) {
        return format!("{body}\\{}", &text[body.len()..]);
    }
    text.to_string()
}

/// Escapes text for a heading (also `{` `}`: heading attributes).
fn escape_heading(s: &str) -> String {
    guard_atx_close(&escape_braces(&escape(s, true)))
}

/// `{` and `}` of already-escaped heading text made literal (a heading may end in `{#id}`), except
/// inside an inline formula `$...$`: its braces are LaTeX. Text never holds a bare `$` (it is
/// escaped to `\$`), so every unescaped one is a formula delimiter.
fn escape_braces(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 4);
    let mut in_math = false;
    let mut it = s.chars();
    while let Some(c) = it.next() {
        match c {
            '\\' => {
                out.push(c);
                if let Some(n) = it.next() {
                    out.push(n);
                }
            }
            '$' => {
                in_math = !in_math;
                out.push(c);
            }
            '{' | '}' if !in_math => {
                out.push('\\');
                out.push(c);
            }
            c => out.push(c),
        }
    }
    out
}

/// `[x](url)` destination: percent-encodes what would end the destination.
fn encode_url(u: &str) -> String {
    let mut out = String::with_capacity(u.len());
    for b in u.bytes() {
        let keep = b.is_ascii_alphanumeric()
            || matches!(
                b,
                b'-' | b'.'
                    | b'_'
                    | b'~'
                    | b':'
                    | b'/'
                    | b'?'
                    | b'#'
                    | b'@'
                    | b'!'
                    | b'$'
                    | b'&'
                    | b'\''
                    | b'*'
                    | b'+'
                    | b','
                    | b';'
                    | b'='
                    | b'%'
            );
        if keep {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// A URL konoma may link to: an allow-list of schemes (`file:`, `javascript:` and friends are
/// text only).
fn safe_url(u: &str) -> Option<String> {
    let u = u.trim();
    if u.is_empty() || u.len() > 2000 {
        return None;
    }
    let lower = u.to_ascii_lowercase();
    let ok = [
        "http://", "https://", "mailto:", "tel:", "ftp://", "ftps://",
    ]
    .iter()
    .any(|p| lower.starts_with(p));
    ok.then(|| encode_url(u))
}

/// GitHub-style heading slug (what konoma's `#anchor` jump matches).
fn slug(text: &str) -> String {
    let mut s = String::new();
    for c in text.trim().chars() {
        if c.is_alphanumeric() {
            s.extend(c.to_lowercase());
        } else if c == ' ' {
            s.push('-');
        } else if c == '-' || c == '_' {
            s.push(c);
        }
    }
    s
}

fn fnv(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for &b in bytes {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

// ---------------------------------------------------------------------------------------------
// inline emission
// ---------------------------------------------------------------------------------------------

enum It<'a> {
    T(String, F),
    O(&'a Seg),
}

fn first_char_class(next: Option<&It<'_>>) -> Option<char> {
    match next {
        Some(It::T(s, _)) => s.chars().next(),
        Some(It::O(Seg::Raw(_) | Seg::Math(_))) => Some('!'),
        _ => None,
    }
}

/// Segments to Markdown (links resolved first). `line_start`: the text begins a line.
fn emit(segs: &[Seg], line_start: bool, cell: bool) -> String {
    let resolved = resolve_links(segs, cell);
    let ce = if cell {
        Some(CellEsc::of(&resolved))
    } else {
        None
    };
    emit_plain(&resolved, line_start, ce, false)
}

fn resolve_links(segs: &[Seg], cell: bool) -> Vec<Seg> {
    if !segs
        .iter()
        .any(|s| matches!(s, Seg::LinkOpen(_) | Seg::LinkClose))
    {
        return segs.to_vec();
    }
    let mut out: Vec<Seg> = Vec::new();
    let mut frame: Option<(String, Vec<Seg>)> = None;
    let close = |f: (String, Vec<Seg>), out: &mut Vec<Seg>| {
        let (url, inner) = f;
        let ce = if cell {
            Some(CellEsc::of(&inner))
        } else {
            None
        };
        let text = emit_plain(&inner, false, ce, true).replace('\n', " ");
        if text.trim().is_empty() {
            return;
        }
        if url.is_empty() {
            out.extend(inner);
        } else {
            out.push(Seg::Raw(format!("[{}]({})", text.trim(), url)));
        }
    };
    for s in segs {
        match s {
            Seg::LinkOpen(u) => {
                if frame.is_none() {
                    frame = Some((u.clone(), Vec::new()));
                }
            }
            Seg::LinkClose => {
                if let Some(f) = frame.take() {
                    close(f, &mut out);
                }
            }
            other => match frame.as_mut() {
                Some(f) => f.1.push(other.clone()),
                None => out.push(other.clone()),
            },
        }
    }
    if let Some(f) = frame.take() {
        close(f, &mut out);
    }
    out
}

fn emit_plain(segs: &[Seg], line_start: bool, ce: Option<CellEsc>, label: bool) -> String {
    let mut items: Vec<It<'_>> = Vec::new();
    for s in segs {
        match s {
            Seg::Text(t, f) => {
                if let Some(It::T(prev, pf)) = items.last_mut() {
                    if pf == f {
                        prev.push_str(t);
                        continue;
                    }
                }
                items.push(It::T(t.clone(), *f));
            }
            other => items.push(It::O(other)),
        }
    }
    let cell = ce.is_some();
    let cev = ce.unwrap_or_default();
    let esc = |t: &str, ls: bool| {
        let e = esc_text(t, ls, cell, cev);
        if label {
            label_fix(&e, cell)
        } else {
            e
        }
    };
    let mut out = String::new();
    // Where each inline formula's closing `$` ends (see the digit guard after the loop).
    let mut math_ends: Vec<usize> = Vec::new();
    for (idx, it) in items.iter().enumerate() {
        let ls = !cell && (line_start && out.is_empty() || out.ends_with('\n'));
        match it {
            It::O(Seg::Raw(r)) => out.push_str(r),
            It::O(Seg::Math(l)) => {
                out.push('$');
                out.push_str(&if cell { cell_math(l) } else { l.clone() });
                out.push('$');
                math_ends.push(out.len());
            }
            It::O(Seg::Break) => {
                while out.ends_with(' ') {
                    out.pop();
                }
                out.push('\n');
            }
            It::O(_) => {}
            It::T(raw, f) => {
                let marks = marks_of(*f, cell);
                if marks.is_empty() && f.v == Vert::Base {
                    let text = if ls {
                        raw.trim_start_matches(char::is_whitespace)
                    } else {
                        raw.as_str()
                    };
                    out.push_str(&esc(text, ls));
                    continue;
                }
                let lead_ws_len = raw.len() - raw.trim_start().len();
                let (lead_ws, rest) = raw.split_at(lead_ws_len);
                let core_end = rest.trim_end().len();
                let (core, trail_ws) = rest.split_at(core_end);
                if core.is_empty() {
                    let t = if ls {
                        raw.trim_start_matches(char::is_whitespace)
                    } else {
                        raw.as_str()
                    };
                    out.push_str(&esc(t, ls));
                    continue;
                }
                let lead_ws = if ls {
                    lead_ws.trim_start_matches(char::is_whitespace)
                } else {
                    lead_ws
                };
                let mut core = core;
                let mut lead_extra = "";
                let mut trail_extra = "";
                let prev = if lead_ws.is_empty() {
                    out.chars().next_back()
                } else {
                    None
                };
                // (Punctuation is moved out of the emphasis markers only; a bare `<sup>` has none.)
                if lead_ws.is_empty() && !marks.is_empty() {
                    if let (Some(p), Some(c)) = (prev, core.chars().next()) {
                        if !p.is_whitespace() && !is_punct(p) && is_punct(c) {
                            lead_extra = &core[..c.len_utf8()];
                            core = &core[c.len_utf8()..];
                        }
                    }
                }
                if trail_ws.is_empty() && !marks.is_empty() {
                    let next = first_char_class(items.get(idx + 1));
                    if let (Some(n), Some(c)) = (next, core.chars().next_back()) {
                        if !n.is_whitespace() && !is_punct(n) && is_punct(c) && !core.is_empty() {
                            trail_extra = &core[core.len() - c.len_utf8()..];
                            core = &core[..core.len() - c.len_utf8()];
                        }
                    }
                }
                if core.is_empty() {
                    out.push_str(&esc(
                        &format!("{lead_ws}{lead_extra}{trail_extra}{trail_ws}"),
                        ls,
                    ));
                    continue;
                }
                let plain_lead = format!("{lead_ws}{lead_extra}");
                if !plain_lead.is_empty() {
                    out.push_str(&esc(&plain_lead, ls));
                }
                out.push_str(marks);
                let (open, close) = match f.v {
                    Vert::Sup => ("<sup>", "</sup>"),
                    Vert::Sub => ("<sub>", "</sub>"),
                    Vert::Base => ("", ""),
                };
                out.push_str(open);
                // The renderer strips `<sup>`/`<sub>` before it reads blocks, so the first
                // character inside a tag at a line start begins the line: it gets the same
                // line-start escapes as plain text (emphasis markers hide it from block parsing).
                // (With no marks, a line-start `ls` leaves nothing before the tag.)
                let core_ls = ls && marks.is_empty() && f.v != Vert::Base;
                out.push_str(&esc(core, core_ls));
                out.push_str(close);
                out.push_str(&marks.chars().rev().collect::<String>());
                let plain_trail = format!("{trail_extra}{trail_ws}");
                if !plain_trail.is_empty() {
                    out.push_str(&esc(&plain_trail, false));
                }
            }
        }
    }
    // The renderer takes `$...$` for a price (`$5 and $10`), not a formula, when a digit follows the
    // closing `$`: a zero-width space keeps the formula a formula. Last to first, so the offsets
    // stay valid.
    for &pos in math_ends.iter().rev() {
        if out[pos..].starts_with(|c: char| c.is_ascii_digit()) {
            out.insert(pos, '\u{200B}');
        }
    }
    // Spaces before a line end would turn into a hard break.
    let mut cleaned = String::with_capacity(out.len());
    let mut first = true;
    for line in out.split('\n') {
        if !first {
            cleaned.push('\n');
        }
        first = false;
        cleaned.push_str(line.trim_end_matches([' ', '\t']));
    }
    cleaned
}

fn marks_of(f: F, cell: bool) -> &'static str {
    // A table cell's scanner is flat: one kind of marker per run, strike only on its own.
    let f = if cell && f.s && (f.b || f.i) {
        F { s: false, ..f }
    } else {
        f
    };
    match (f.s, f.b, f.i) {
        (false, false, false) => "",
        (false, true, false) => "**",
        (false, false, true) => "*",
        (false, true, true) => "***",
        (true, false, false) => "~~",
        (true, true, false) => "~~**",
        (true, false, true) => "~~*",
        (true, true, true) => "~~***",
    }
}

/// The text of `segs` as konoma will draw it in a heading, which is what its `#anchor` slug is
/// made of: a footnote mark `[^3]` is drawn as a superscript digit (a digit to the slug), a
/// formula as its LaTeX source (konoma draws it raw in a heading, and the slug drops the
/// punctuation).
fn plain_of(segs: &[Seg]) -> String {
    let mut s = String::new();
    for g in segs {
        match g {
            Seg::Text(t, f) => match f.v {
                // What the renderer draws for `<sup>2</sup>` is `²` (a different letter to the slug).
                Vert::Sup => s.push_str(&vertical_chars(t, sup_char)),
                Vert::Sub => s.push_str(&vertical_chars(t, sub_char)),
                Vert::Base => s.push_str(t),
            },
            Seg::Math(t) => s.push_str(t),
            Seg::Break => s.push(' '),
            Seg::Raw(r) => {
                if let Some(n) = r
                    .strip_prefix("[^")
                    .and_then(|r| r.strip_suffix(']'))
                    .and_then(|n| n.parse::<usize>().ok())
                {
                    s.push_str(&superscript(n));
                }
            }
            _ => {}
        }
    }
    s
}

/// Superscript form of a character, as konoma's renderer draws `<sup>` (kept in step with
/// `markdown.rs::sup_char` by a test).
fn sup_char(c: char) -> Option<char> {
    Some(match c {
        '0'..='9' => [
            '\u{2070}', '\u{b9}', '\u{b2}', '\u{b3}', '\u{2074}', '\u{2075}', '\u{2076}',
            '\u{2077}', '\u{2078}', '\u{2079}',
        ][c as usize - '0' as usize],
        '+' => '\u{207a}',
        '-' => '\u{207b}',
        '=' => '\u{207c}',
        '(' => '\u{207d}',
        ')' => '\u{207e}',
        'n' => '\u{207f}',
        'i' => '\u{2071}',
        _ => return None,
    })
}

/// Subscript form of a character (see [`sup_char`]).
fn sub_char(c: char) -> Option<char> {
    Some(match c {
        '0'..='9' => [
            '\u{2080}', '\u{2081}', '\u{2082}', '\u{2083}', '\u{2084}', '\u{2085}', '\u{2086}',
            '\u{2087}', '\u{2088}', '\u{2089}',
        ][c as usize - '0' as usize],
        '+' => '\u{208a}',
        '-' => '\u{208b}',
        '=' => '\u{208c}',
        '(' => '\u{208d}',
        ')' => '\u{208e}',
        _ => return None,
    })
}

/// `t` in the vertical form when every character has one, else unchanged (what the renderer does).
fn vertical_chars(t: &str, f: impl Fn(char) -> Option<char>) -> String {
    // (Whitespace around the text is written outside the tags, so it is not part of the match.)
    let core = t.trim();
    let lead = &t[..t.len() - t.trim_start().len()];
    let trail = &t[t.trim_end().len()..];
    match core.chars().map(f).collect::<Option<String>>() {
        Some(m) if !core.is_empty() => format!("{lead}{m}{trail}"),
        _ => t.to_string(),
    }
}

/// Test hook: the vertical form of `t` the slug computation uses.
#[cfg(test)]
pub(super) fn vertical_for_test(t: &str, sup: bool) -> String {
    vertical_chars(t, if sup { sup_char } else { sub_char })
}

/// The text of `segs` without anything that is not a character of the heading (a note mark, a
/// formula): the name an OpenDocument cross reference gives the heading (`#Title|outline`).
fn outline_text(segs: &[Seg]) -> String {
    let mut s = String::new();
    for g in segs {
        match g {
            Seg::Text(t, _) => s.push_str(t),
            Seg::Break => s.push(' '),
            _ => {}
        }
    }
    s
}

/// `12` as superscript digits (what the renderer draws for the footnote mark `[^12]`).
fn superscript(n: usize) -> String {
    const SUP: [char; 10] = [
        '\u{2070}', '\u{b9}', '\u{b2}', '\u{b3}', '\u{2074}', '\u{2075}', '\u{2076}', '\u{2077}',
        '\u{2078}', '\u{2079}',
    ];
    n.to_string()
        .chars()
        .map(|c| SUP[c.to_digit(10).unwrap_or(0) as usize])
        .collect()
}

// ---------------------------------------------------------------------------------------------
// blocks to Markdown
// ---------------------------------------------------------------------------------------------

fn code_span(t: &str) -> String {
    // The cell scanner reads `...` to the next backtick and has no escape: no backtick inside.
    let t = t.replace(['\n', '\r'], " ").replace('`', "\u{02CB}");
    let longest = t.split(|c| c != '`').map(str::len).max().unwrap_or(0);
    let fence = "`".repeat(longest + 1);
    if longest > 0 || t.starts_with('`') || t.ends_with('`') {
        format!("{fence} {t} {fence}")
    } else {
        format!("{fence}{t}{fence}")
    }
}

/// A block as one inline string (a table cell or a footnote). `br` joins lines.
fn cell_text(blks: &[Blk], br: &str) -> String {
    blk_strings(blks, br, false)
        .into_iter()
        .map(|(s, _)| s)
        .collect::<Vec<_>>()
        .join(br)
}

/// The body of a footnote / endnote as the text after `[^n]: `. konoma's footnote reader takes a
/// definition of one paragraph with indented continuation lines (a blank line followed by an
/// indented paragraph is drawn as code and the whole note is left unnumbered), so the paragraphs
/// of a note are lines of it, each on its own line: a backslash hard break (the reader trims the
/// two spaces of the other kind off the first line) and the four-space indent.
fn note_text(blks: &[Blk]) -> String {
    const BREAK: &str = "\\\n    ";
    blk_strings(blks, "\n", true)
        .into_iter()
        .map(|(s, _)| s.replace('\n', BREAK))
        .collect::<Vec<_>>()
        .join(BREAK)
}

/// The blocks as inline strings, one per non-empty block (`br` joins the lines of a block); the
/// flag says the block is a list item. `note`: the text goes to a footnote, whose lines the
/// Markdown reader parses (a `1.` at a line start is escaped), not to a table cell.
fn blk_strings(blks: &[Blk], br: &str, note: bool) -> Vec<(String, bool)> {
    let mut parts: Vec<(String, bool)> = Vec::new();
    for b in blks {
        let s = match b {
            Blk::Heading { text, .. } => format!("**{}**", text.replace('\n', " ")),
            Blk::Para(t) => t.replace('\n', br),
            Blk::Item(it) => {
                let text = it.text.replace('\n', br);
                match &it.marker {
                    Marker::Bullet => format!("\u{2022} {text}"),
                    Marker::Ordered(n) if note => format!("{n}\\. {text}"),
                    Marker::Ordered(n) => format!("{n}. {text}"),
                    Marker::Literal if note => format!("{} {text}", escape(&it.label, true)),
                    Marker::Literal => format!(
                        "{} {text}",
                        esc_text(&it.label, false, true, CellEsc::default())
                    ),
                }
            }
            Blk::Code(t) => code_span(&t.replace(NBSP, " ")),
            Blk::Table(rows) => rows
                .iter()
                .map(|r| {
                    r.iter()
                        .filter(|c| !c.is_empty())
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(" / ")
                })
                .filter(|r| !r.is_empty())
                .collect::<Vec<_>>()
                .join(br),
            Blk::Math(l) => format!("${l}$"),
        };
        if !s.trim().is_empty() {
            parts.push((s.trim().to_string(), matches!(b, Blk::Item(_))));
        }
    }
    parts
}

/// A formula for a table cell: no `|` (the converters write `\vert`; this is the net for what
/// they pass through, such as a `|` of a `\text{}` that has none).
fn cell_math(latex: &str) -> String {
    latex.replace('|', "\u{2223}")
}

/// The most bytes of chart parts read for titles over a whole document.
const CHART_READ_BUDGET: u64 = 4 * 1024 * 1024;

// Test probe: the most bytes any buffer that gathers a document's text before it is written
// (code paragraphs, note definitions, table cells) held at once. The budget tests assert on it:
// the output of a document does not change when such a buffer is bounded, only the memory does.
#[cfg(test)]
thread_local! {
    pub(super) static PEAK_HELD: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Records `_bytes` held by a gathering buffer (a no-op outside tests).
#[inline]
pub(super) fn held(_bytes: usize) {
    #[cfg(test)]
    PEAK_HELD.with(|p| p.set(p.get().max(_bytes)));
}

/// The length of the longest run of consecutive backticks in `s`.
fn longest_backtick_run(s: &str) -> usize {
    let (mut best, mut cur) = (0usize, 0usize);
    for c in s.chars() {
        if c == '`' {
            cur += 1;
            best = best.max(cur);
        } else {
            cur = 0;
        }
    }
    best
}

fn render_table(rows: &[Vec<String>]) -> String {
    let cols = rows.iter().map(Vec::len).max().unwrap_or(0);
    if cols == 0 {
        return String::new();
    }
    let line = |r: &Vec<String>| {
        let mut s = String::from("|");
        for c in 0..cols {
            let cell = r.get(c).map(String::as_str).unwrap_or("");
            s.push(' ');
            s.push_str(if cell.is_empty() { " " } else { cell });
            s.push_str(" |");
        }
        s
    };
    let mut out = line(&rows[0]);
    out.push('\n');
    out.push('|');
    for _ in 0..cols {
        out.push_str(" --- |");
    }
    for r in &rows[1..] {
        out.push('\n');
        out.push_str(&line(r));
    }
    out
}

impl<'a> Conv<'a> {
    pub(super) fn cancelled(&self) -> bool {
        self.cancel.is_some_and(Cancel::is_cancelled)
    }

    // -----------------------------------------------------------------------------------------
    // body
    // -----------------------------------------------------------------------------------------

    fn read_body(&mut self, src: impl BufRead) -> Result<(), OfficeError> {
        let mut rd = XmlReader::new(src);
        let mut buf = Vec::new();
        // The main part's root must be `w:document`: a package whose officeDocument relationship
        // names something else (an xlsx / pptx renamed .docx) is not a Word document.
        let mut root_seen = false;
        loop {
            buf.clear();
            match rd.read_event_into(&mut buf).map_err(xml_err)? {
                Event::Start(e) | Event::Empty(e) if !root_seen => {
                    root_seen = true;
                    if e.local_name().as_ref() != b"document" {
                        return Err(OfficeError::Unsupported);
                    }
                }
                Event::Start(e) if e.local_name().as_ref() == b"body" => break,
                Event::Eof => return Ok(()),
                _ => {}
            }
        }
        let mut count = 0usize;
        loop {
            buf.clear();
            let ev = rd.read_event_into(&mut buf).map_err(xml_err)?;
            let (e, empty) = match ev {
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
            count += 1;
            if count > self.opts.max_blocks || self.cancelled() || self.full {
                self.truncated = true;
                return Ok(());
            }
            let mut budget = Budget::new(self.opts.max_block_nodes, self.opts.max_block_text_bytes);
            match read_element(&mut rd, &e, empty, &mut budget)? {
                Tree::Ok(node) => {
                    let mut blks = Vec::new();
                    self.block_node(&node, Ctx::Body, 0, &mut blks);
                    self.write_blocks(blks);
                }
                Tree::TooBig => {
                    self.truncated = true;
                    return Ok(());
                }
            }
            if self.full {
                self.truncated = true;
                return Ok(());
            }
        }
    }

    fn read_notes(
        &mut self,
        src: impl BufRead,
        note_rels: HashMap<String, Rel>,
        is_end: bool,
        defs: &mut Vec<(usize, String)>,
    ) -> Result<(), OfficeError> {
        let tag: &[u8] = if is_end { b"endnote" } else { b"footnote" };
        let saved = std::mem::replace(&mut self.rels, note_rels);
        self.in_note = true;
        let mut rd = XmlReader::new(src);
        let mut buf = Vec::new();
        let mut result = Ok(());
        let mut wanted = self.note_labels.iter().filter(|l| l.0 == is_end).count();
        while wanted > 0 {
            buf.clear();
            let ev = match rd.read_event_into(&mut buf).map_err(xml_err) {
                Ok(ev) => ev,
                Err(e) => {
                    result = Err(e);
                    break;
                }
            };
            let (e, empty) = match ev {
                Event::Start(e) => (e.into_owned(), false),
                Event::Empty(e) => (e.into_owned(), true),
                Event::Eof => break,
                _ => continue,
            };
            if e.local_name().as_ref() != tag {
                continue;
            }
            let mut budget = Budget::new(self.opts.max_block_nodes, self.opts.max_block_text_bytes);
            let node = match read_element(&mut rd, &e, empty, &mut budget) {
                Ok(Tree::Ok(n)) => n,
                Ok(Tree::TooBig) => {
                    self.truncated = true;
                    continue;
                }
                Err(e) => {
                    result = Err(e);
                    break;
                }
            };
            if matches!(
                node.attr("type"),
                Some("separator" | "continuationSeparator" | "continuationNotice")
            ) {
                continue;
            }
            let Some(id) = node.attr("id").and_then(|v| v.trim().parse::<i64>().ok()) else {
                continue;
            };
            let Some(&pos) = self.note_index.get(&(is_end, id)) else {
                continue;
            };
            wanted -= 1;
            let mut blks = Vec::new();
            self.blocks(&node.kids, Ctx::Note, 0, &mut blks);
            self.flush_carry(&mut blks, Ctx::Note);
            let mut text = note_text(&blks);
            if text.trim().is_empty() {
                text = "\u{2014}".into();
            }
            if !self.keep_note(text.len()) {
                break;
            }
            defs.push((pos + 1, text));
            if self.cancelled() {
                self.truncated = true;
                break;
            }
        }
        self.in_note = false;
        self.rels = saved;
        result
    }

    /// Whether a note definition of `len` bytes may be held (all of them together stay within the
    /// output's own limit: 5,000 notes of 40 KB each would otherwise be 200 MB that the end of the
    /// document throws away). The conversion is marked truncated once it says no.
    pub(super) fn keep_note(&mut self, len: usize) -> bool {
        if self.defs_bytes.saturating_add(len) > self.opts.max_markdown_bytes {
            self.truncated = true;
            return false;
        }
        self.defs_bytes += len;
        held(self.defs_bytes);
        true
    }

    // -----------------------------------------------------------------------------------------
    // writer
    // -----------------------------------------------------------------------------------------

    fn flush_code(&mut self) {
        if let Some(code) = self.pending_code.take() {
            // The longest run of backticks anywhere (a closing fence may be indented up to three
            // spaces, so a run is a danger wherever it stands on its line).
            let longest = longest_backtick_run(&code);
            let fence = "`".repeat((longest + 1).max(3));
            let piece = format!("{fence}\n{code}\n{fence}");
            self.push_piece(piece, Last::Other, "\n\n");
        }
    }

    fn write_blocks(&mut self, blks: Vec<Blk>) {
        for b in blks {
            if self.full {
                return;
            }
            self.write_block(b);
        }
    }

    fn write_block(&mut self, b: Blk) {
        if let Blk::Code(t) = &b {
            let t = t.replace(NBSP, " ");
            // The code paragraphs wait here until the run ends: they count against the output
            // budget like anything written (a document of a million code paragraphs must not be
            // held whole).
            let waiting = self.pending_code.as_ref().map_or(0, |c| c.len() + 1);
            if self.out.len() + waiting + t.len() > self.body_bytes {
                self.flush_code();
                self.full = true;
                self.truncated = true;
                return;
            }
            match &mut self.pending_code {
                Some(c) => {
                    c.push('\n');
                    c.push_str(&t);
                    held(c.len());
                }
                None => {
                    held(t.len());
                    self.pending_code = Some(t);
                }
            }
            return;
        }
        self.flush_code();
        match b {
            Blk::Code(_) => {}
            Blk::Heading {
                level,
                text,
                plain,
                bookmarks,
            } => {
                self.register_heading(&plain, &bookmarks);
                let piece = format!("{} {}", "#".repeat(usize::from(level.clamp(1, 6))), text);
                self.stack.clear();
                self.last_list = None;
                self.push_piece(piece, Last::Other, "\n\n");
            }
            Blk::Para(t) => {
                self.stack.clear();
                self.last_list = None;
                let t = t.replace('\n', "  \n");
                self.push_piece(t, Last::Other, "\n\n");
            }
            Blk::Math(l) => {
                self.stack.clear();
                self.last_list = None;
                self.push_piece(
                    format!("$$\n{}\n$$", guard_display(&l)),
                    Last::Other,
                    "\n\n",
                );
            }
            Blk::Table(rows) => {
                self.stack.clear();
                self.last_list = None;
                let t = render_table(&rows);
                if !t.is_empty() {
                    self.push_piece(t, Last::Other, "\n\n");
                }
            }
            Blk::Item(it) => self.write_item(it),
        }
    }

    fn write_item(&mut self, it: Item) {
        match &it.marker {
            Marker::Literal => {
                self.stack.clear();
                self.last_list = None;
                let indent: String = std::iter::repeat_n(NBSP, 2 * usize::from(it.level)).collect();
                let text = it.text.replace('\n', "  \n");
                let piece = format!("{indent}{} {text}", escape(&it.label, true));
                // (Literal items of separate lists of a slide are separate paragraphs.)
                let same_list = !self.split_bullet_lists || self.last_literal == Some(it.list);
                let sep = if self.last == Last::Literal && same_list {
                    "  \n"
                } else {
                    "\n\n"
                };
                self.last_literal = Some(it.list);
                self.push_piece(piece, Last::Literal, sep);
            }
            Marker::Bullet | Marker::Ordered(_) => {
                while self.stack.last().is_some_and(|&(l, _)| l >= it.level) {
                    self.stack.pop();
                }
                let indent = self.stack.last().map_or(0, |&(_, w)| w);
                let marker = match &it.marker {
                    Marker::Ordered(n) => format!("{n}. "),
                    _ => "- ".to_string(),
                };
                let ordered = matches!(it.marker, Marker::Ordered(_));
                let content_indent = indent + marker.len();
                let pad = " ".repeat(content_indent);
                let text = it.text.replace('\n', &format!("  \n{pad}"));
                let piece = format!("{}{marker}{text}", " ".repeat(indent));
                let sep = if self.last == Last::Real {
                    match self.last_list {
                        Some((l, o)) if l == it.list && o == ordered => "\n",
                        // Two lists of the same kind side by side would be read as one (and
                        // numbered on): an empty HTML comment ends the first.
                        Some((_, o))
                            if o == ordered
                                && (ordered || self.split_bullet_lists)
                                && indent == 0 =>
                        {
                            "\n\n<!-- -->\n\n"
                        }
                        _ => "\n",
                    }
                } else {
                    "\n\n"
                };
                self.stack.push((it.level, content_indent));
                self.last_list = Some((it.list, ordered));
                self.push_piece(piece, Last::Real, sep);
            }
        }
    }

    /// Appends a block, within the budgets. A block that does not fit is cut at a line boundary
    /// (or dropped) and the conversion is over.
    fn push_piece(&mut self, piece: String, kind: Last, sep: &str) {
        if self.full {
            return;
        }
        let sep = if self.last == Last::None { "" } else { sep };
        let mut text = format!("{sep}{piece}");
        let add_lines = text.matches('\n').count() + usize::from(self.last == Last::None);
        let over_bytes = self.out.len() + text.len() > self.body_bytes;
        let over_lines = self.out_lines + add_lines > self.body_lines;
        if over_bytes || over_lines {
            self.full = true;
            self.truncated = true;
            // Keep as much of the block as fits, whole lines only.
            let room_bytes = self.body_bytes.saturating_sub(self.out.len());
            let room_lines = self.body_lines.saturating_sub(self.out_lines);
            let mut keep = String::new();
            let mut lines = 0usize;
            for (i, l) in text.split('\n').enumerate() {
                let extra = if i == 0 { 0 } else { 1 };
                if keep.len() + extra + l.len() > room_bytes || lines + extra > room_lines {
                    break;
                }
                if i > 0 {
                    keep.push('\n');
                    lines += 1;
                }
                keep.push_str(l);
            }
            // Never keep a block's separator without its first line.
            if keep.trim().is_empty() || keep.len() <= sep.len() {
                // One line longer than all the room left: keep its beginning (a document that is
                // one huge paragraph still shows something).
                if self.out.is_empty() && room_bytes > 0 && room_lines > 0 {
                    let mut n = room_bytes.min(piece.len());
                    while !piece.is_char_boundary(n) {
                        n -= 1;
                    }
                    let first = &piece[..piece[..n].find('\n').unwrap_or(n)];
                    if !first.trim().is_empty() {
                        self.out.push_str(first);
                        self.out_lines += 1;
                        self.last = kind;
                    }
                }
                return;
            }
            text = keep;
        }
        self.out_lines += text.matches('\n').count() + usize::from(self.last == Last::None);
        self.out.push_str(&text);
        self.last = kind;
    }

    fn register_heading(&mut self, plain: &str, bookmarks: &[String]) {
        let base = slug(plain);
        if base.is_empty() {
            return;
        }
        let n = self.slug_counts.entry(base.clone()).or_insert(0);
        let s = if *n == 0 {
            base.clone()
        } else {
            format!("{base}-{n}")
        };
        *n += 1;
        for b in bookmarks {
            self.bookmark_slugs
                .entry(b.clone())
                .or_insert_with(|| s.clone());
        }
    }

    /// Replaces the anchor tokens in link destinations with heading slugs.
    fn resolve_anchors(&self, md: String) -> String {
        if !md.contains(TOKEN_OPEN) {
            return md;
        }
        let mut out = String::with_capacity(md.len());
        let mut rest = md.as_str();
        while let Some(i) = rest.find(TOKEN_OPEN) {
            out.push_str(&rest[..i]);
            let after = &rest[i + TOKEN_OPEN.len_utf8()..];
            let Some(j) = after.find(TOKEN_CLOSE) else {
                rest = after;
                continue;
            };
            let idx: usize = after[..j].parse().unwrap_or(usize::MAX);
            if let Some(name) = self.anchor_names.get(idx) {
                // The slug as written: konoma matches `#anchor` against heading slugs
                // character for character (a slug holds only letters, digits, `-` and `_`).
                match self.bookmark_slugs.get(name) {
                    Some(s) => out.push_str(s),
                    // (`|outline`: an OpenDocument cross reference to a heading nobody registered.)
                    None => out.push_str(&slug(name.strip_suffix("|outline").unwrap_or(name))),
                }
            }
            rest = &after[j + TOKEN_CLOSE.len_utf8()..];
        }
        out.push_str(rest);
        out
    }

    // -----------------------------------------------------------------------------------------
    // blocks
    // -----------------------------------------------------------------------------------------

    fn flush_carry_top(&mut self) {
        let mut blks = Vec::new();
        self.flush_carry(&mut blks, Ctx::Body);
        self.write_blocks(blks);
    }

    /// A paragraph whose mark was deleted joins the next one; with no next paragraph it stands alone.
    fn flush_carry(&mut self, out: &mut Vec<Blk>, ctx: Ctx) {
        if self.carry.is_empty() {
            return;
        }
        let segs = std::mem::take(&mut self.carry);
        self.carry_seen = (0, false);
        let role = Role {
            heading: None,
            code: false,
            list: None,
        };
        let mut b = self.make_blocks(segs, &role, Vec::new(), ctx);
        out.append(&mut b);
    }

    fn blocks(&mut self, kids: &[Kid], ctx: Ctx, depth: usize, out: &mut Vec<Blk>) {
        if depth > 40 {
            // Whatever stood below the limit is dropped: say so.
            self.truncated |= kids.iter().any(|k| matches!(k, Kid::N(_)));
            return;
        }
        for k in kids {
            if let Kid::N(n) = k {
                self.block_node(n, ctx, depth, out);
            }
        }
    }

    fn block_node(&mut self, n: &Node, ctx: Ctx, depth: usize, out: &mut Vec<Blk>) {
        if depth > 40 {
            self.truncated = true;
            return;
        }
        match n.name.as_str() {
            "p" => self.para(n, ctx, depth, out),
            "tbl" => {
                self.flush_carry(out, ctx);
                if let Some(t) = self.table(n, depth) {
                    out.push(t);
                }
            }
            "sdt" => {
                if let Some(c) = n.child("sdtContent") {
                    self.blocks(&c.kids, ctx, depth + 1, out);
                }
            }
            "customXml" | "smartTag" | "ins" | "moveTo" | "dir" | "bdo" | "sdtContent" => {
                self.blocks(&n.kids, ctx, depth + 1, out)
            }
            "AlternateContent" => {
                if let Some(c) = alt_content(n, DOCX_READS) {
                    self.blocks(&c.kids, ctx, depth + 1, out);
                }
            }
            // An embedded document (HTML, RTF, another docx) that Word merges into the page: not
            // read here, so say that it stood here.
            "altChunk" => {
                self.flush_carry(out, ctx);
                let name = n
                    .rel_attr("id")
                    .and_then(|id| self.rels.get(id))
                    .map(|r| r.target.rsplit('/').next().unwrap_or("").to_string())
                    .unwrap_or_default();
                let name: String = clean(&name).chars().take(80).collect();
                self.cur_ctx = ctx;
                let label = if name.trim().is_empty() {
                    "embedded document".to_string()
                } else {
                    format!("embedded document: {}", name.trim())
                };
                out.push(Blk::Para(self.placeholder(&label)));
            }
            _ => {}
        }
    }

    fn table(&mut self, t: &Node, depth: usize) -> Option<Blk> {
        if depth > 12 {
            self.truncated = true;
            return None;
        }
        let mut rows: Vec<Vec<String>> = Vec::new();
        let mut cells = 0usize;
        let mut trs: Vec<&Node> = Vec::new();
        collect_rows(t, &mut trs, 0);
        'rows: for tr in trs {
            if tr.child("trPr").and_then(|p| p.child("del")).is_some() {
                continue;
            }
            let mut row: Vec<String> = Vec::new();
            // `w:gridBefore`: the row starts that many grid columns in (no cells before it). The
            // columns are empty cells so the others stay under their own columns. (`w:gridAfter`
            // needs nothing: a short row is padded when the table is written.)
            let before = tr
                .child("trPr")
                .and_then(|p| p.child("gridBefore"))
                .and_then(|g| g.attr("val"))
                .and_then(|v| v.trim().parse::<usize>().ok())
                .unwrap_or(0)
                .min(64);
            cells += before;
            if cells > self.opts.max_table_cells {
                self.truncated = true;
                break 'rows;
            }
            row.resize(before, String::new());
            let mut tcs: Vec<&Node> = Vec::new();
            collect_cells(tr, &mut tcs, 0);
            for tc in tcs {
                let pr = tc.child("tcPr");
                let span = pr
                    .and_then(|p| p.child("gridSpan"))
                    .and_then(|g| g.attr("val"))
                    .and_then(|v| v.trim().parse::<usize>().ok())
                    .unwrap_or(1)
                    .clamp(1, 64);
                let covered = |name: &str| {
                    pr.and_then(|p| p.child(name))
                        .is_some_and(|m| m.attr("val") != Some("restart"))
                };
                let text = if covered("vMerge") || covered("hMerge") {
                    String::new()
                } else {
                    let mut blks = Vec::new();
                    self.blocks(&tc.kids, Ctx::Cell, depth + 1, &mut blks);
                    self.flush_carry(&mut blks, Ctx::Cell);
                    cell_text(&blks, "<br>")
                };
                cells += span;
                if cells > self.opts.max_table_cells {
                    self.truncated = true;
                    break 'rows;
                }
                row.push(text);
                for _ in 1..span {
                    row.push(String::new());
                }
            }
            // A row of no cells is no row, whatever columns `w:gridBefore` skips.
            if row.len() > before {
                rows.push(row);
            }
        }
        (!rows.is_empty()).then_some(Blk::Table(rows))
    }

    // -----------------------------------------------------------------------------------------
    // paragraphs
    // -----------------------------------------------------------------------------------------

    fn para(&mut self, p: &Node, ctx: Ctx, depth: usize, out: &mut Vec<Blk>) {
        self.cur_ctx = ctx;
        let ppr = p.child("pPr");
        let style_id = ppr
            .and_then(|n| n.child("pStyle"))
            .and_then(|n| n.attr("val"));
        let ps = self.styles.para(style_id);
        let direct_outline = ppr
            .and_then(|n| n.child("outlineLvl"))
            .and_then(|n| n.attr("val"))
            .and_then(|v| v.trim().parse::<u8>().ok());
        let heading = match direct_outline {
            Some(v) => (v < 9).then_some(v + 1),
            None => ps.heading,
        };
        let numpr = match ppr.and_then(direct_num) {
            Some((0, _)) => None,
            Some(x) => Some(x),
            None => ps.num.filter(|n| n.0 != 0),
        };
        let mark_deleted = ppr
            .and_then(|n| n.child("rPr"))
            .is_some_and(|r| r.child("del").is_some() || r.child("moveFrom").is_some());

        let mut base = ps.fmt;
        if heading.is_some() {
            base.bold = None;
            base.italic = None;
        }
        let mut inl = Inl {
            segs: std::mem::take(&mut self.carry),
            ..Inl::default()
        };
        // A field open from an earlier paragraph measures its result from this paragraph's start.
        let saved: Vec<usize> = self.fields.iter().map(|f| f.result_from).collect();
        for f in &mut self.fields {
            f.result_from = inl.segs.len();
        }
        self.inline_children(p, base, &mut inl, depth);
        for (i, f) in self.fields.iter_mut().enumerate() {
            if f.phase == Phase::Result && inl.segs.len() > f.result_from {
                f.has_result = true;
            }
            if let Some(old) = saved.get(i) {
                f.result_from = *old;
            }
        }
        for f in &mut self.fields {
            if f.link_open {
                inl.segs.push(Seg::LinkClose);
                f.link_open = false;
            }
        }
        let Inl {
            segs,
            extras,
            bookmarks,
        } = inl;
        if mark_deleted && extras.is_empty() {
            self.carry = segs;
            return;
        }

        // Numbering advances for every numbered paragraph, shown or not.
        let list = numpr.and_then(|(num_id, ilvl)| self.number(num_id, ilvl, &ps.ids));
        let role = Role {
            heading,
            code: ps.code && heading.is_none() && list.is_none(),
            list,
        };
        let mut blks = self.make_blocks(segs, &role, bookmarks, ctx);
        out.append(&mut blks);
        out.extend(extras);
    }

    /// Advances the list counters for a paragraph of list `num_id` at `ilvl`. `None`: not a list
    /// the reader knows.
    fn number(&mut self, num_id: u32, ilvl: Option<u8>, style_ids: &[String]) -> Option<ListRole> {
        if !self.numbering.has_num(num_id) {
            return None;
        }
        let aid = self.numbering.abstract_of(num_id, &self.styles)?;
        let lvl = ilvl
            .or_else(|| self.numbering.level_for_style(aid, style_ids))
            .unwrap_or(0)
            .min(8);
        let first = self.seen_nums.insert(num_id);
        let counters = self.counters.entry(aid).or_insert([None; 9]);
        if first {
            for l in 0..9u8 {
                if let Some(s) = self.numbering.start_override(num_id, l) {
                    counters[usize::from(l)] = Some(i64::from(s) - 1);
                }
            }
        }
        let def = self.numbering.level(num_id, aid, lvl);
        let li = usize::from(lvl);
        let value = match counters[li] {
            None => i64::from(def.start),
            Some(v) => v + 1,
        };
        counters[li] = Some(value);
        for c in counters.iter_mut().skip(li + 1) {
            *c = None;
        }
        let snapshot = *counters;
        if def.fmt == "bullet" {
            return Some(ListRole {
                level: lvl,
                list: num_id,
                marker: Marker::Bullet,
                label: String::new(),
            });
        }
        if def.fmt == "none" {
            return None;
        }
        let numbering = &self.numbering;
        let label = render_label(&def.text, |k| {
            let d = numbering.level(num_id, aid, k as u8);
            let v = snapshot[k].unwrap_or(i64::from(d.start)).max(0) as u32;
            let fmt = if def.legal { "decimal" } else { d.fmt.as_str() };
            format_number(fmt, v)
        });
        let real =
            def.fmt == "decimal" && !def.legal && def.text == format!("%{}.", li + 1) && value >= 0;
        let marker = if real {
            Marker::Ordered(value.min(999_999_999) as u32)
        } else {
            Marker::Literal
        };
        Some(ListRole {
            level: lvl,
            list: num_id,
            marker,
            label: clean(&label),
        })
    }

    /// Splits a paragraph's segments at page breaks and display formulas and builds its blocks.
    fn make_blocks(
        &mut self,
        segs: Vec<Seg>,
        role: &Role,
        bookmarks: Vec<String>,
        ctx: Ctx,
    ) -> Vec<Blk> {
        let mut parts: Vec<Result<Vec<Seg>, Result<String, String>>> = Vec::new();
        let mut cur: Vec<Seg> = Vec::new();
        for s in segs {
            match s {
                Seg::PageBreak => {
                    parts.push(Ok(std::mem::take(&mut cur)));
                }
                Seg::Display(d) => {
                    parts.push(Ok(std::mem::take(&mut cur)));
                    parts.push(Err(d));
                }
                other => cur.push(other),
            }
        }
        parts.push(Ok(cur));
        let mut out = Vec::new();
        let mut used = false;
        let mut bookmarks = Some(bookmarks);
        for part in parts {
            match part {
                Err(d) => match d {
                    Ok(latex) => out.push(if ctx.flat() {
                        let latex = if ctx == Ctx::Cell {
                            cell_math(&latex)
                        } else {
                            latex
                        };
                        Blk::Para(format!("${latex}$"))
                    } else {
                        Blk::Math(latex)
                    }),
                    Err(text) => {
                        let t = esc_text(&text, true, ctx == Ctx::Cell, CellEsc::of_str(&text));
                        if !t.trim().is_empty() {
                            out.push(Blk::Para(t));
                        }
                    }
                },
                Ok(segs) => {
                    let heading_role = !used && role.heading.is_some();
                    let text = emit(&segs, true, ctx == Ctx::Cell);
                    if text.trim_matches(|c: char| c.is_whitespace()).is_empty() {
                        continue;
                    }
                    if role.code {
                        used = true;
                        out.push(Blk::Code(plain_code(&segs)));
                        continue;
                    }
                    if heading_role {
                        used = true;
                        let level = role.heading.unwrap_or(1);
                        let mut plain = plain_of(&segs).trim().to_string();
                        let mut text = emit(&segs, false, ctx == Ctx::Cell)
                            .replace('\n', " ")
                            .trim()
                            .to_string();
                        text = escape_braces(&text);
                        let mut label = None;
                        if let Some(l) = &role.list {
                            if l.marker != Marker::Bullet && !l.label.trim().is_empty() {
                                label = Some(l.label.clone());
                            }
                        }
                        if let Some(l) = label {
                            plain = format!("{l} {plain}");
                            text = format!("{} {text}", escape_heading(&l));
                        }
                        let text = guard_atx_close(&text);
                        if ctx.flat() {
                            out.push(Blk::Heading {
                                level,
                                text,
                                plain,
                                bookmarks: Vec::new(),
                            });
                        } else {
                            out.push(Blk::Heading {
                                level,
                                text,
                                plain,
                                bookmarks: bookmarks.take().unwrap_or_default(),
                            });
                        }
                        continue;
                    }
                    if !used {
                        used = true;
                        if let Some(l) = &role.list {
                            out.push(Blk::Item(Item {
                                level: l.level,
                                list: l.list,
                                marker: l.marker.clone(),
                                label: l.label.clone(),
                                text: guard_rule(text.trim().to_string()),
                            }));
                            continue;
                        }
                    }
                    out.push(Blk::Para(guard_rule(text.trim().to_string())));
                }
            }
        }
        out
    }

    // -----------------------------------------------------------------------------------------
    // inline content
    // -----------------------------------------------------------------------------------------

    fn in_instr(&self) -> bool {
        self.fields.iter().any(|f| f.phase == Phase::Instr)
    }

    fn push_text(&self, inl: &mut Inl, s: &str, f: Fmt) {
        if self.in_instr() || s.is_empty() {
            return;
        }
        let fmt = F {
            b: f.bold.unwrap_or(false),
            i: f.italic.unwrap_or(false),
            s: f.strike.unwrap_or(false),
            v: f.vert.unwrap_or_default(),
        };
        inl.segs.push(Seg::Text(s.to_string(), fmt));
    }

    fn inline_children(&mut self, parent: &Node, base: Fmt, inl: &mut Inl, depth: usize) {
        if depth > 60 {
            self.truncated |= parent.nodes().next().is_some();
            return;
        }
        for n in parent.nodes() {
            self.inline_node(n, base, inl, depth);
        }
    }

    fn inline_node(&mut self, n: &Node, base: Fmt, inl: &mut Inl, depth: usize) {
        if depth > 60 {
            self.truncated = true;
            return;
        }
        match n.name.as_str() {
            "r" => self.run(n, base, inl, depth),
            "hyperlink" => {
                let url = self.hyperlink_dest(n);
                inl.segs.push(Seg::LinkOpen(url));
                self.inline_children(n, base, inl, depth + 1);
                inl.segs.push(Seg::LinkClose);
            }
            "fldSimple" => {
                let instr = n.attr("instr").unwrap_or("");
                let url = hyperlink_instr(instr).map(|h| self.field_dest(h));
                if let Some(u) = &url {
                    inl.segs.push(Seg::LinkOpen(u.clone()));
                }
                self.inline_children(n, base, inl, depth + 1);
                if url.is_some() {
                    inl.segs.push(Seg::LinkClose);
                }
            }
            "ruby" => self.ruby(n, base, inl, depth + 1),
            "ins" | "moveTo" | "smartTag" | "customXml" | "dir" | "bdo" | "sdtContent" => {
                self.inline_children(n, base, inl, depth + 1)
            }
            "sdt" => {
                if let Some(c) = n.child("sdtContent") {
                    self.inline_children(c, base, inl, depth + 1);
                }
            }
            "AlternateContent" => {
                if let Some(c) = alt_content(n, DOCX_READS) {
                    self.inline_children(c, base, inl, depth + 1);
                }
            }
            "bookmarkStart" => {
                if let Some(name) = n.attr("name") {
                    if !name.starts_with("_GoBack") && inl.bookmarks.len() < 64 {
                        inl.bookmarks.push(name.to_string());
                    }
                }
            }
            "oMath" => self.math(n, false, inl),
            "oMathPara" => self.math(n, true, inl),
            _ => {}
        }
    }

    /// `w:ruby`: the base text, then its reading in brackets (full-width after a CJK base, so
    /// `漢字（かんじ）`; the reading is text the reader would otherwise lose).
    fn ruby(&mut self, n: &Node, fmt: Fmt, inl: &mut Inl, depth: usize) {
        if depth > 60 {
            self.truncated = true;
            return;
        }
        let start = inl.segs.len();
        if let Some(b) = n.child("rubyBase") {
            self.inline_children(b, fmt, inl, depth + 1);
        }
        let mut reading = String::new();
        if let Some(rt) = n.child("rt") {
            collect_t_text(rt, &mut reading, 0);
        }
        let last = inl.segs[start..].iter().rev().find_map(|g| match g {
            Seg::Text(t, _) => t.chars().next_back(),
            _ => None,
        });
        self.push_reading(inl, &reading, last, fmt);
    }

    /// The reading of a ruby, bracketed after its base text (`last`: the base's last character).
    fn push_reading(&self, inl: &mut Inl, reading: &str, last: Option<char>, fmt: Fmt) {
        let reading = clean(reading);
        let reading = reading.trim();
        if reading.is_empty() || self.in_instr() {
            return;
        }
        let text = if last.is_some_and(is_cjk) {
            format!("\u{FF08}{reading}\u{FF09}")
        } else {
            format!(" ({reading})")
        };
        self.push_text(inl, &text, fmt);
    }

    fn math(&mut self, n: &Node, display: bool, inl: &mut Inl) {
        if self.in_instr() {
            return;
        }
        self.math_total += 1;
        // The converter's check cannot be interrupted, so a cancelled load tries no more of them.
        let xml = if self.cancelled() {
            None
        } else {
            to_xml(n, self.opts.max_math_xml)
        };
        let latex = xml
            .as_deref()
            .and_then(|x| (self.opts.math)(x, display))
            .map(|l| omml::tidy(&l.replace(['\n', '\r'], " ")))
            .filter(|l| !l.is_empty() && !omml::has_bare_dollar(l) && !l.contains(NBSP));
        // What shows when it cannot be drawn: the formula in a linear form (fractions, scripts,
        // operators with limits keep their shape); the bare characters when it is too big to read.
        let text = clean(
            &xml.as_deref()
                .map(omml::fallback_text)
                .filter(|t| !t.trim().is_empty())
                .unwrap_or_else(|| math_text(n, 0)),
        );
        if latex.is_some() {
            self.math_latex += 1;
        }
        if display {
            inl.segs.push(Seg::Display(latex.ok_or(text)));
        } else {
            match latex {
                Some(l) => inl.segs.push(Seg::Math(l)),
                None => inl.segs.push(Seg::Text(text, F::default())),
            }
        }
    }

    fn hyperlink_dest(&mut self, n: &Node) -> String {
        if let Some(id) = n.rel_attr("id") {
            if let Some(rel) = self.rels.get(id) {
                if rel.external {
                    return safe_url(&rel.target).unwrap_or_default();
                }
            }
        }
        match n.attr("anchor") {
            Some(a) if !a.is_empty() => self.anchor_dest(a),
            _ => String::new(),
        }
    }

    fn field_dest(&mut self, h: HyperlinkInstr) -> String {
        match h {
            HyperlinkInstr::Url(u) => safe_url(&u).unwrap_or_default(),
            HyperlinkInstr::Anchor(a) => self.anchor_dest(&a),
        }
    }

    fn anchor_dest(&mut self, name: &str) -> String {
        let idx = match self.anchor_index.get(name) {
            Some(&i) => i,
            None => {
                if self.anchor_names.len() >= 100_000 {
                    return String::new();
                }
                self.anchor_names.push(name.to_string());
                self.anchor_index
                    .insert(name.to_string(), self.anchor_names.len() - 1);
                self.anchor_names.len() - 1
            }
        };
        format!("#{TOKEN_OPEN}{idx}{TOKEN_CLOSE}")
    }

    fn run(&mut self, r: &Node, base: Fmt, inl: &mut Inl, depth: usize) {
        let rpr = r.child("rPr");
        let mut fmt = base;
        if let Some(rpr) = rpr {
            if let Some(id) = rpr.child("rStyle").and_then(|s| s.attr("val")) {
                fmt = fmt.over(self.styles.char_fmt(id));
            }
            fmt = fmt.over(Fmt::from_rpr(rpr));
        }
        // Hidden by the run itself, its character style, its paragraph style or the document
        // defaults (`w:vanish`, with `w:val="0"` switching an inherited one off).
        let hidden = fmt.hidden.unwrap_or(false)
            || rpr.is_some_and(|p| p.toggle("specVanish").unwrap_or(false));
        self.run_children(r, fmt, hidden, inl, depth);
    }

    fn run_children(&mut self, r: &Node, fmt: Fmt, hidden: bool, inl: &mut Inl, depth: usize) {
        if depth > 60 {
            self.truncated |= r.nodes().next().is_some();
            return;
        }
        for c in r.nodes() {
            match c.name.as_str() {
                "t" => {
                    if hidden {
                        continue;
                    }
                    let raw = c.text();
                    let t = if c.attr("space") == Some("preserve") {
                        raw
                    } else {
                        raw.trim().to_string()
                    };
                    let t = clean(&map_run_text(&t, fmt.font));
                    self.push_text(inl, &t, fmt);
                }
                "tab" | "ptab" => {
                    if !hidden {
                        let tab: String = std::iter::repeat_n(NBSP, 4).collect();
                        self.push_text(inl, &tab, Fmt::default());
                    }
                }
                "br" => {
                    if hidden || self.in_instr() {
                        continue;
                    }
                    match c.attr("type") {
                        Some("page") | Some("column") => inl.segs.push(Seg::PageBreak),
                        _ => inl.segs.push(Seg::Break),
                    }
                }
                "cr" => {
                    if !hidden && !self.in_instr() {
                        inl.segs.push(Seg::Break);
                    }
                }
                "noBreakHyphen" => {
                    if !hidden {
                        self.push_text(inl, "\u{2011}", fmt);
                    }
                }
                "sym" => {
                    if !hidden {
                        // `w:sym`: a character of a (symbol) font. Mapped to what it draws; one the
                        // tables do not know is a visible substitute, never nothing.
                        if let Some(ch) = c
                            .attr("char")
                            .and_then(|v| u32::from_str_radix(v.trim(), 16).ok())
                            .and_then(char::from_u32)
                            .filter(|&ch| ch >= ' ')
                        {
                            let ch = map_sym(c.attr("font").unwrap_or(""), ch);
                            let s = clean(&ch.to_string());
                            self.push_text(inl, &s, fmt);
                        }
                    }
                }
                "fldChar" => self.fld_char(c, inl, hidden),
                "instrText" => {
                    if let Some(f) = self.fields.last_mut() {
                        if f.phase == Phase::Instr && f.instr.len() < 4000 {
                            f.instr.push_str(&c.text());
                        }
                    }
                }
                "footnoteReference" | "endnoteReference" => {
                    if hidden || self.in_note || self.in_instr() {
                        continue;
                    }
                    let is_end = c.name == "endnoteReference";
                    let Some(id) = c.attr("id").and_then(|v| v.trim().parse::<i64>().ok()) else {
                        continue;
                    };
                    let key = (is_end, id);
                    let n = match self.note_index.get(&key) {
                        Some(&p) => p + 1,
                        None => {
                            if self.note_labels.len() >= self.opts.max_notes {
                                self.truncated = true;
                                continue;
                            }
                            self.note_labels.push(key);
                            self.note_index.insert(key, self.note_labels.len() - 1);
                            self.note_labels.len()
                        }
                    };
                    inl.segs.push(Seg::Raw(format!("[^{n}]")));
                }
                "drawing" | "pict" | "object" => {
                    if !hidden && !self.in_instr() {
                        self.media_node(c, inl, depth);
                    }
                }
                "AlternateContent" => {
                    if let Some(ch) = alt_content(c, DOCX_READS) {
                        self.run_children(ch, fmt, hidden, inl, depth + 1);
                    }
                }
                "ruby" if !hidden => self.ruby(c, fmt, inl, depth + 1),
                _ => {}
            }
        }
    }

    fn fld_char(&mut self, c: &Node, inl: &mut Inl, hidden: bool) {
        match c.attr("fldCharType") {
            Some("begin") => {
                if self.fields.len() < 64 {
                    self.fields.push(Field {
                        instr: String::new(),
                        phase: Phase::Instr,
                        link_open: false,
                        form: c.child("ffData").and_then(form_field_text),
                        result_from: 0,
                        has_result: false,
                        hidden,
                    });
                }
            }
            Some("separate") => {
                let dest = match self.fields.last() {
                    Some(f) if f.phase == Phase::Instr => hyperlink_instr(&f.instr),
                    _ => None,
                };
                let dest = dest.map(|h| self.field_dest(h));
                if let Some(f) = self.fields.last_mut() {
                    f.phase = Phase::Result;
                    f.result_from = inl.segs.len();
                    if let Some(d) = dest {
                        f.link_open = true;
                        inl.segs.push(Seg::LinkOpen(d));
                    }
                }
            }
            Some("end") => {
                if let Some(f) = self.fields.pop() {
                    if f.link_open {
                        inl.segs.push(Seg::LinkClose);
                    }
                    // A form field with no result of its own is drawn from its form data: a check
                    // box as its box, a drop-down as the chosen entry.
                    let empty = f.phase == Phase::Instr
                        || !(f.has_result || inl.segs.len() > f.result_from);
                    if let (true, false, Some(text)) = (empty, f.hidden || hidden, &f.form) {
                        self.push_text(inl, text, Fmt::default());
                    }
                }
            }
            _ => {}
        }
    }

    // -----------------------------------------------------------------------------------------
    // pictures and text boxes
    // -----------------------------------------------------------------------------------------

    fn media_node(&mut self, n: &Node, inl: &mut Inl, depth: usize) {
        let mut found = Media::default();
        scan_media(n, &mut found, 0);
        self.truncated |= found.cut;
        let alt = clean(&found.alt);
        let alt: String = alt.chars().take(300).collect();
        for rid in &found.embeds {
            let md = self.image_md(rid, &alt);
            inl.segs.push(Seg::Raw(md));
        }
        if found.embeds.is_empty()
            && found.textboxes.is_empty()
            && (found.linked > 0 || !alt.trim().is_empty())
        {
            let md = self.placeholder(&alt);
            inl.segs.push(Seg::Raw(md));
        } else if found.embeds.is_empty() && found.textboxes.is_empty() {
            // A chart or a SmartArt graphic has no picture in the file and often no description:
            // show that something stood here (the chart's title when it has one).
            let label = match found.graphic {
                Some(Graphic::Chart) => {
                    let title = found.chart_rid.as_deref().and_then(|r| self.chart_title(r));
                    Some(match title {
                        Some(t) => format!("chart: {t}"),
                        None => "chart".to_string(),
                    })
                }
                Some(Graphic::SmartArt) => Some("SmartArt".to_string()),
                // An embedded object whose picture is missing: say what stood here.
                _ => found.ole.as_deref().map(|id| {
                    if id.is_empty() {
                        "object".to_string()
                    } else {
                        format!("object: {id}")
                    }
                }),
            };
            if let Some(l) = label {
                let md = self.placeholder(&l);
                inl.segs.push(Seg::Raw(md));
            }
        }
        for tb in &found.textboxes {
            if depth > 30 {
                break;
            }
            let mut blks = Vec::new();
            let ctx = self.cur_ctx;
            self.blocks(&tb.kids, ctx, depth + 8, &mut blks);
            inl.extras.append(&mut blks);
        }
    }

    /// The title of the chart stored in the part `rid` names (`c:title` text), bounded: each part
    /// is read once (a chart drawn many times shares the answer) and a document's chart parts
    /// together are read up to [`CHART_READ_BUDGET`] bytes.
    fn chart_title(&mut self, rid: &str) -> Option<String> {
        let rel = self.rels.get(rid)?;
        if rel.external {
            return None;
        }
        let part = rel.target.clone();
        if let Some(t) = self.chart_titles.get(&part) {
            return t.clone();
        }
        let t = self.read_chart_title(&part);
        self.chart_titles.insert(part, t.clone());
        t
    }

    fn read_chart_title(&mut self, part: &str) -> Option<String> {
        let left = CHART_READ_BUDGET.saturating_sub(self.chart_read);
        if left == 0 || self.cancelled() {
            return None;
        }
        let limit = left.min(512 * 1024);
        let mut bytes = Vec::new();
        {
            let r = self.media.part(part, limit).ok()??;
            r.take(limit).read_to_end(&mut bytes).ok()?;
        }
        self.chart_read += bytes.len() as u64;
        let mut rd = XmlReader::new(&bytes[..]);
        let mut buf = Vec::new();
        // Elements open: `c:chartSpace` (1) > `c:chart` (2) > `c:title`: the chart's own title, not
        // the title of an axis or a series.
        let mut depth = 0usize;
        loop {
            buf.clear();
            let ev = rd.read_event_into(&mut buf).ok()?;
            let (e, empty) = match ev {
                Event::Start(e) => (e.into_owned(), false),
                Event::Empty(e) => (e.into_owned(), true),
                Event::End(_) => {
                    depth = depth.saturating_sub(1);
                    continue;
                }
                Event::Eof => return None,
                _ => continue,
            };
            if e.local_name().as_ref() != b"title" || depth != 2 {
                if !empty {
                    depth += 1;
                }
                continue;
            }
            let mut budget = Budget::new(5_000, 16 * 1024);
            let Ok(Tree::Ok(node)) = read_element(&mut rd, &e, empty, &mut budget) else {
                return None;
            };
            let mut text = String::new();
            collect_t_text(&node, &mut text, 0);
            let text: String = clean(&text)
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            let text: String = text.chars().take(200).collect();
            return (!text.is_empty()).then_some(text);
        }
    }

    fn placeholder(&self, alt: &str) -> String {
        let label = if alt.trim().is_empty() {
            "image"
        } else {
            alt.trim()
        };
        let t = format!("[{label}]");
        esc_text(&t, false, self.cur_ctx == Ctx::Cell, CellEsc::of_str(&t))
    }

    fn image_md(&mut self, rid: &str, alt: &str) -> String {
        let Some(rel) = self.rels.get(rid).cloned() else {
            return self.placeholder(alt);
        };
        if rel.external {
            // konoma never fetches a picture a document points at.
            return self.placeholder(alt);
        }
        self.image_md_part(&rel.target, alt)
    }

    /// `![alt](office-img://..)` for the picture stored at `part` of the package (a placeholder when
    /// it cannot be shown).
    fn image_md_part(&mut self, part: &str, alt: &str) -> String {
        let alt_md = alt.replace(
            [
                '[', ']', '(', ')', '<', '>', '\\', '`', '*', '_', '$', '~', '|', '&', '!',
            ],
            " ",
        );
        let alt_md = alt_md.split_whitespace().collect::<Vec<_>>().join(" ");
        let key = match self.image_by_part.get(part) {
            Some(k) => k.clone(),
            None => {
                let k = self.load_image(part);
                self.image_by_part.insert(part.to_string(), k.clone());
                k
            }
        };
        match key {
            Some(k) => format!("![{alt_md}]({k})"),
            None => self.placeholder(alt),
        }
    }

    fn load_image(&mut self, part: &str) -> Option<String> {
        let name = part.rsplit('/').next().unwrap_or(part).to_string();
        let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase())?;
        if !matches!(
            ext.as_str(),
            "png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp" | "svg" | "tif" | "tiff"
        ) {
            return None;
        }
        if self.images.len() >= self.opts.max_images {
            self.truncated = true;
            return None;
        }
        let cap = self.opts.max_image_bytes;
        let mut bytes = Vec::new();
        {
            let r = self.media.part(part, cap + 1).ok()??;
            r.take(cap + 1).read_to_end(&mut bytes).ok()?;
        }
        if bytes.len() as u64 > cap || bytes.is_empty() {
            if bytes.len() as u64 > cap {
                self.truncated = true;
            }
            return None;
        }
        if self.image_total + bytes.len() as u64 > self.opts.max_total_image_bytes {
            self.truncated = true;
            return None;
        }
        let safe: String = name
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        let key = format!(
            "office-img://{:012x}/{safe}",
            fnv(&bytes) & 0xffff_ffff_ffff
        );
        if self.images.iter().any(|i| i.key == key) {
            return Some(key);
        }
        self.image_total += bytes.len() as u64;
        self.images.push(DocImage {
            key: key.clone(),
            bytes,
            name,
        });
        Some(key)
    }
}

// ---------------------------------------------------------------------------------------------
// free helpers
// ---------------------------------------------------------------------------------------------

/// The `t` text (`w:t`, `a:t`) under `n`, in order.
fn collect_t_text(n: &Node, out: &mut String, depth: usize) {
    if depth > 64 || out.len() > 4096 {
        return;
    }
    for k in &n.kids {
        match k {
            Kid::T(t) if n.name == "t" => out.push_str(t),
            Kid::T(_) => {}
            Kid::N(c) => collect_t_text(c, out, depth + 1),
        }
    }
}

fn plain_code(segs: &[Seg]) -> String {
    let mut s = String::new();
    for g in segs {
        match g {
            Seg::Text(t, _) => s.push_str(t),
            Seg::Break => s.push('\n'),
            Seg::Raw(_) => {}
            _ => {}
        }
    }
    s.trim_end().to_string()
}

fn math_text(n: &Node, depth: usize) -> String {
    let mut s = String::new();
    if depth > 100 {
        return s;
    }
    for k in &n.kids {
        match k {
            Kid::T(t) => {
                if n.name == "t" {
                    s.push_str(t);
                }
            }
            Kid::N(c) => s.push_str(&math_text(c, depth + 1)),
        }
    }
    s
}

/// The namespaces (as the prefixes Word writes them) whose content the Word reader reads inside an
/// `mc:Choice`: the text boxes, groups and canvases of a drawing (`wps`, `wpg`, `wpc`), the
/// extensions of the drawing and of the text (`wp14`, `w14`, `w15`, `w16*`), formulas (`m`, `a14`)
/// and the always-understood ones. Anything else (chart extensions `cx`, ink `p14`, ..) is
/// something the reader cannot show, so the `Fallback` (usually a picture of it) is read instead.
const DOCX_READS: &[&str] = &[
    "w", "wp", "a", "pic", "r", "m", "v", "o", "wps", "wpg", "wpc", "wp14", "w14", "w15", "w16",
    "w16se", "w16cid", "w16du", "w16sdtdh", "w16sdtfl", "a14",
];

/// The part of an `mc:AlternateContent` to read (ECMA-376 part 3, Markup Compatibility): the first
/// `Choice` whose `Requires` namespaces are **all** ones the reader understands (`reads`: their
/// prefixes, as the producing application writes them), else the `Fallback`; `None` when there is
/// neither. A `Choice` with no `Requires` requires nothing.
fn alt_content<'n>(n: &'n Node, reads: &[&str]) -> Option<&'n Node> {
    let readable = |c: &Node| {
        c.attr("Requires")
            .is_none_or(|r| r.split_whitespace().all(|p| reads.contains(&p)))
    };
    n.nodes()
        .find(|c| c.name == "Choice" && readable(c))
        .or_else(|| n.child("Fallback"))
}

fn collect_rows<'n>(t: &'n Node, out: &mut Vec<&'n Node>, depth: usize) {
    if depth > 4 {
        return;
    }
    for c in t.nodes() {
        match c.name.as_str() {
            "tr" => out.push(c),
            "sdt" => {
                if let Some(content) = c.child("sdtContent") {
                    collect_rows(content, out, depth + 1);
                }
            }
            "ins" | "customXml" => collect_rows(c, out, depth + 1),
            _ => {}
        }
    }
}

fn collect_cells<'n>(tr: &'n Node, out: &mut Vec<&'n Node>, depth: usize) {
    if depth > 4 {
        return;
    }
    for c in tr.nodes() {
        match c.name.as_str() {
            "tc" => out.push(c),
            "sdt" => {
                if let Some(content) = c.child("sdtContent") {
                    collect_cells(content, out, depth + 1);
                }
            }
            "ins" | "customXml" => collect_cells(c, out, depth + 1),
            _ => {}
        }
    }
}

#[derive(Default)]
struct Media<'n> {
    alt: String,
    embeds: Vec<String>,
    /// Pictures that point outside the package (never fetched).
    linked: usize,
    textboxes: Vec<&'n Node>,
    /// The `a:graphicData` kinds found (a chart, SmartArt ...).
    graphic: Option<Graphic>,
    /// The relationship of the chart part (`c:chart r:id`).
    chart_rid: Option<String>,
    /// An embedded object (`o:OLEObject`): its `ProgID` (`Excel.Sheet.12`), empty when it has none.
    ole: Option<String>,
    /// The scan stopped at the depth limit with elements still unread.
    cut: bool,
}

/// What a `drawing`'s `a:graphicData` holds when it is not a picture.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Graphic {
    Chart,
    SmartArt,
    Other,
}

fn graphic_of(uri: &str) -> Graphic {
    let u = uri.to_ascii_lowercase();
    // `.../drawingml/2006/chart`, and the 2014 `chartex` (waterfall, funnel ...).
    if u.contains("/chart") {
        Graphic::Chart
    } else if u.ends_with("/diagram") {
        Graphic::SmartArt
    } else {
        Graphic::Other
    }
}

fn scan_media<'n>(n: &'n Node, m: &mut Media<'n>, depth: usize) {
    if depth > 64 {
        m.cut |= n.nodes().next().is_some();
        return;
    }
    for c in n.nodes() {
        match c.name.as_str() {
            "docPr" => {
                if m.alt.is_empty() {
                    let d = c.attr("descr").filter(|d| !d.trim().is_empty());
                    let t = c.attr("title").filter(|d| !d.trim().is_empty());
                    m.alt = d.or(t).unwrap_or("").to_string();
                }
            }
            "graphicData" => {
                if let Some(g) = c.attr("uri").map(graphic_of) {
                    if m.graphic.is_none() || g != Graphic::Other {
                        m.graphic = Some(g);
                    }
                }
                scan_media(c, m, depth + 1);
            }
            "OLEObject" => {
                if m.ole.is_none() {
                    let id = c.attr("ProgID").map(clean).unwrap_or_default();
                    m.ole = Some(id.trim().chars().take(80).collect());
                }
            }
            "chart" => {
                if m.chart_rid.is_none() {
                    m.chart_rid = c.rel_attr("id").map(str::to_string);
                }
            }
            "blip" => {
                if let Some(id) = c.rel_attr("embed") {
                    if m.embeds.len() < 32 {
                        m.embeds.push(id.to_string());
                    }
                } else if c.rel_attr("link").is_some() {
                    m.linked += 1;
                }
            }
            "shape" => {
                if m.alt.is_empty() {
                    m.alt = c.attr("alt").unwrap_or("").to_string();
                }
                scan_media(c, m, depth + 1);
            }
            "imagedata" => {
                if m.alt.is_empty() {
                    m.alt = c.attr("title").unwrap_or("").to_string();
                }
                let id = c.rel_attr("id").or_else(|| c.rel_attr("relid"));
                if let Some(id) = id {
                    if m.embeds.len() < 32 {
                        m.embeds.push(id.to_string());
                    }
                }
            }
            "txbxContent" => {
                if m.textboxes.len() < 32 {
                    m.textboxes.push(c);
                }
            }
            "AlternateContent" => {
                if let Some(ch) = alt_content(c, DOCX_READS) {
                    scan_media(ch, m, depth + 1);
                }
            }
            _ => scan_media(c, m, depth + 1),
        }
    }
}

enum HyperlinkInstr {
    Url(String),
    Anchor(String),
}

/// `HYPERLINK "url"` / `HYPERLINK \l "bookmark"` field instructions.
fn hyperlink_instr(instr: &str) -> Option<HyperlinkInstr> {
    let mut tokens: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut in_q = false;
    for c in instr.chars() {
        match c {
            '"' => {
                in_q = !in_q;
                if !in_q {
                    tokens.push(std::mem::take(&mut cur));
                }
            }
            c if c.is_whitespace() && !in_q => {
                if !cur.is_empty() {
                    tokens.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        tokens.push(cur);
    }
    let mut it = tokens.into_iter();
    if !it.next()?.eq_ignore_ascii_case("HYPERLINK") {
        return None;
    }
    let mut url: Option<String> = None;
    let mut anchor: Option<String> = None;
    while let Some(t) = it.next() {
        match t.as_str() {
            "\\l" | "\\L" => anchor = it.next(),
            "\\o" | "\\O" | "\\t" | "\\T" | "\\m" | "\\M" => {
                it.next();
            }
            s if s.starts_with('\\') => {}
            _ => {
                if url.is_none() {
                    url = Some(t);
                }
            }
        }
    }
    match (url, anchor) {
        (Some(u), Some(a)) if !a.is_empty() => Some(HyperlinkInstr::Url(format!("{u}#{a}"))),
        (Some(u), _) => Some(HyperlinkInstr::Url(u)),
        (None, Some(a)) if !a.is_empty() => Some(HyperlinkInstr::Anchor(a)),
        _ => None,
    }
}

#[cfg(test)]
mod tests_docx_unit {
    use super::*;

    #[test]
    fn escape_basics() {
        assert_eq!(
            escape("a*b_c [x] <y> $5 | `z`", false),
            "a\\*b_c \\[x] \\<y> \\$5 \\| \\`z\\`"
        );
        assert_eq!(escape("1. item", true), "1\\. item");
        assert_eq!(escape("- x", true), "\\- x");
        assert_eq!(escape("#tag", true), "\\#tag");
        assert_eq!(escape("a - b", false), "a - b");
    }
}
