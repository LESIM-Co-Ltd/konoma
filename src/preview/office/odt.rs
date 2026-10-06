//! OpenDocument text preview (odt / ott): the reading side. The same job as the docx reader and
//! the same output (a Markdown string plus its pictures, [`Document`]), built on the same
//! converter: this module is a child of `docx.rs` so it shares its writer (blocks, lists, tables,
//! escaping for konoma's renderer, notes, pictures, anchors) and only supplies what is ODF-specific:
//! the style model, the numbering model and the walk over `content.xml`.
//!
//! # What is converted
//!
//! | OpenDocument | Markdown |
//! |---|---|
//! | `text:h` (`text:outline-level`), a `text:p` whose style (chain) has `style:default-outline-level` or is named `Heading n` / `Title` | `#` .. `######` |
//! | bold / italic / strike (`fo:font-weight`, `fo:font-style`, `style:text-line-through-*`, through `style:parent-style-name`) | `**` `*` `~~` |
//! | superscript / subscript (`style:text-position`: `super 58%`, `sub`, a raise in percent) | `<sup>` / `<sub>` |
//! | `text:ruby` | the base text, then its reading in brackets |
//! | `text:line-break`, `text:tab`, `text:s`, `text:soft-page-break` | hard break, four no-break spaces, spaces, nothing |
//! | `text:list` with a bullet level | a real `- ` list (nested) |
//! | `text:list` with decimal `N.` numbering | a real `N. ` list **with the number the document shows** (`text:start-value`, continued lists) |
//! | every other number format (letters, roman, `ア` `①` `一`, prefix / suffix, `1.1`, `loext:num-list-format`) | a paragraph that starts with the document's own label text |
//! | `table:table` | GFM table (spans: value top-left, `table:covered-table-cell` empty; repeats capped; nested tables flattened) |
//! | `draw:frame` > `draw:image` (`Pictures/..`) | `![alt](office-img://<hash>/<name>)` + [`super::DocImage`] |
//! | `draw:frame` > `draw:object` (LibreOffice Math, MathML) | `$latex$` / `$$latex$$` via `mathml::to_latex`; the StarMath source when it cannot convert |
//! | `text:note` (footnote / endnote) | `[^n]` + `[^n]: text` |
//! | `text:a` | `[text](url)` (an allow-list of schemes) / `[text](#heading-slug)` for `#bookmark` and `#Heading|outline` |
//! | `text:table-of-content`, fields (`text:page-number` ..) | their stored text |
//! | `draw:text-box`, shapes with text | their paragraphs, after the paragraph holding them |
//! | `text:section` | its content (a section with `text:display="none"` is not shown) |
//!
//! Tracked changes are shown as the **final** version: LibreOffice keeps deleted text inside
//! `text:tracked-changes` (never read) and leaves a `text:change` mark in the body; text between
//! `text:change-start` and `text:change-end` of a *deletion* region (other producers) is dropped.
//! A deletion that runs from the end of one paragraph into the next (the break between them was deleted)
//! joins the two, as LibreOffice shows them.
//! Comments (`office:annotation`), headers and footers (master pages in `styles.xml`, never read) and
//! hidden text (`text:display="none"`) are not shown.
//!
//! # Differences from the docx reader
//!
//! * The text of a paragraph is the text of the XML (whitespace collapses to one space as ODF
//!   says; `text:s` is the only way to keep several).
//! * A list is *structure* (`text:list` nested in `text:list-item`), numbered by the list style of
//!   its outermost list, so counters live per list instance (`text:continue-list`,
//!   `text:continue-numbering`, `xml:id`), not per `numId`.
//! * Footnotes are inline (`text:note-body`), so they are converted where they are referenced.
//! * A merged cell is followed by an explicit `table:covered-table-cell`; nothing is added for spans.
//! * A formula is not markup in the text but an embedded object (`Object N/content.xml`).
//!
//! # Safety
//!
//! Same as docx: the package goes through [`container`] first, every XML part through `XmlReader`
//! (depth 256), the body is read **one top-level block at a time** (sections and indexes are
//! entered, not held) with a node and text budget per block, and all the Markdown / picture / table
//! budgets of [`DocOptions`] apply. A damaged `styles.xml` costs the styles, not the document.

use super::super::docx_styles::{heading_from_name, is_code_name, Vert};
use super::super::docx_xml::skip_rest;
use super::*;
use crate::preview::office::{mathml, omml};

/// Longest object directory name followed (bytes).
const OBJECT_DIR_MAX: usize = 512;

/// `style:family` values the converter reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Fam {
    Para,
    Text,
}

#[derive(Debug, Clone, Default)]
struct OdStyle {
    parent: Option<String>,
    /// The name a user sees (`style:display-name`, else the decoded `style:name`).
    display: String,
    outline: Option<u8>,
    fmt: Fmt,
    hidden: Option<bool>,
}

/// One level of a list style (or of the outline style).
#[derive(Debug, Clone)]
struct LevelDef {
    /// `false`: a bullet or image level.
    number: bool,
    /// `style:num-format` as written (`1`, `a`, `ア, イ, ウ, ...`).
    format: String,
    letter_sync: bool,
    prefix: String,
    suffix: String,
    start: i64,
    display_levels: usize,
    /// `loext:num-list-format` (`%1%.`): the whole label with a placeholder per level.
    list_format: Option<String>,
}

#[derive(Debug, Default)]
struct OdStyles {
    styles: HashMap<(Fam, String), OdStyle>,
    lists: HashMap<String, Vec<Option<LevelDef>>>,
    outline: Vec<Option<LevelDef>>,
}

/// What a paragraph style (with everything it is based on) amounts to.
#[derive(Debug, Default)]
struct OdPara {
    heading: Option<u8>,
    fmt: Fmt,
    code: bool,
    hidden: bool,
}

/// Deepest element nesting the inline walk follows (the XML reader refuses more than 256).
const INLINE_DEPTH: usize = 220;
const MAX_CHAIN: usize = 32;
const MAX_STYLES: usize = 50_000;

/// `Heading_20_1` -> `Heading 1`: ODF writes a character that is not allowed in a name as `_HEX_`.
fn decode_name(name: &str) -> String {
    let b: Vec<char> = name.chars().collect();
    let mut out = String::with_capacity(name.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == '_' {
            let mut j = i + 1;
            while j < b.len() && j - i <= 5 && b[j].is_ascii_hexdigit() {
                j += 1;
            }
            if j < b.len() && b[j] == '_' && (3..=5).contains(&(j - i)) {
                let hex: String = b[i + 1..j].iter().collect();
                if let Some(c) = u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
                    out.push(c);
                    i = j + 1;
                    continue;
                }
            }
        }
        out.push(b[i]);
        i += 1;
    }
    out
}

/// `fo:font-weight` and friends: bold from 600 up.
fn bold_of(v: &str) -> Option<bool> {
    match v.trim() {
        "bold" => Some(true),
        "normal" => Some(false),
        n => n.parse::<u32>().ok().map(|w| w >= 600),
    }
}

fn italic_of(v: &str) -> Option<bool> {
    match v.trim() {
        "italic" | "oblique" => Some(true),
        "normal" => Some(false),
        _ => None,
    }
}

/// `style:text-properties` -> the formatting Markdown can express, and whether the text is hidden.
fn text_props(tp: &Node) -> (Fmt, Option<bool>) {
    let bold = tp
        .attr("font-weight")
        .and_then(bold_of)
        .or_else(|| tp.attr("font-weight-asian").and_then(bold_of));
    let italic = tp
        .attr("font-style")
        .and_then(italic_of)
        .or_else(|| tp.attr("font-style-asian").and_then(italic_of));
    let line = [
        tp.attr("text-line-through-style"),
        tp.attr("text-line-through-type"),
    ];
    let strike = if line.iter().flatten().any(|v| v.trim() == "none") {
        Some(false)
    } else if line.iter().flatten().next().is_some() {
        Some(true)
    } else {
        None
    };
    let hidden = tp.attr("display").map(|v| v.trim() == "none");
    (
        Fmt {
            bold,
            italic,
            strike,
            vert: tp.attr("text-position").and_then(vert_of),
            ..Fmt::default()
        },
        hidden,
    )
}

/// `style:text-position`: `super 58%`, `sub 58%`, or a raise in percent (`33% 58%`, `-25%`):
/// above the baseline is superscript, below it subscript, `0%` the baseline itself.
fn vert_of(v: &str) -> Option<Vert> {
    let first = v.split_whitespace().next()?;
    match first {
        "super" => return Some(Vert::Sup),
        "sub" => return Some(Vert::Sub),
        _ => {}
    }
    let pct: f64 = first.strip_suffix('%')?.trim().parse().ok()?;
    Some(if pct > 0.0 {
        Vert::Sup
    } else if pct < 0.0 {
        Vert::Sub
    } else {
        Vert::Base
    })
}

fn level_def(l: &Node) -> LevelDef {
    let number = l.name == "list-level-style-number" || l.name == "outline-level-style";
    LevelDef {
        number,
        format: l.attr("num-format").unwrap_or("").to_string(),
        letter_sync: l
            .attr("num-letter-sync")
            .is_some_and(|v| v.trim() == "true"),
        prefix: l.attr("num-prefix").unwrap_or("").to_string(),
        suffix: l.attr("num-suffix").unwrap_or("").to_string(),
        start: l
            .attr("start-value")
            .and_then(|v| v.trim().parse::<i64>().ok())
            .unwrap_or(1)
            .clamp(0, 999_999_999),
        display_levels: l
            .attr("display-levels")
            .and_then(|v| v.trim().parse::<usize>().ok())
            .unwrap_or(1)
            .clamp(1, 10),
        list_format: l.attr("num-list-format").map(str::to_string),
    }
}

fn levels_of(n: &Node) -> Vec<Option<LevelDef>> {
    let mut levels: Vec<Option<LevelDef>> = vec![None; 10];
    for l in n.nodes() {
        if !matches!(
            l.name.as_str(),
            "list-level-style-number"
                | "list-level-style-bullet"
                | "list-level-style-image"
                | "outline-level-style"
        ) {
            continue;
        }
        let Some(lv) = l
            .attr("level")
            .and_then(|v| v.trim().parse::<usize>().ok())
            .filter(|v| (1..=10).contains(v))
        else {
            continue;
        };
        levels[lv - 1] = Some(level_def(l));
    }
    levels
}

impl OdStyles {
    /// Reads the styles of an `office:styles` / `office:automatic-styles` element.
    fn add(&mut self, container: &Node) {
        for n in container.nodes() {
            match n.name.as_str() {
                "style" => {
                    let fam = match n.attr("family") {
                        Some("paragraph") => Fam::Para,
                        Some("text") => Fam::Text,
                        _ => continue,
                    };
                    let Some(name) = n.attr("name") else { continue };
                    if self.styles.len() >= MAX_STYLES {
                        continue;
                    }
                    let mut d = OdStyle {
                        parent: n.attr("parent-style-name").map(str::to_string),
                        display: n
                            .attr("display-name")
                            .map(str::to_string)
                            .unwrap_or_else(|| decode_name(name)),
                        outline: n
                            .attr("default-outline-level")
                            .and_then(|v| v.trim().parse::<u8>().ok())
                            .filter(|v| (1..=10).contains(v)),
                        ..OdStyle::default()
                    };
                    if let Some(tp) = n.child("text-properties") {
                        let (f, h) = text_props(tp);
                        d.fmt = f;
                        d.hidden = h;
                    }
                    self.styles.insert((fam, name.to_string()), d);
                }
                "list-style" => {
                    if let Some(name) = n.attr("name") {
                        if self.lists.len() < MAX_STYLES {
                            self.lists.insert(name.to_string(), levels_of(n));
                        }
                    }
                }
                "outline-style" => self.outline = levels_of(n),
                _ => {}
            }
        }
    }

    /// The styles of the chain starting at `name`, most derived first (a cycle ends the chain).
    fn chain(&self, fam: Fam, name: &str) -> Vec<(&str, &OdStyle)> {
        let mut out: Vec<(&str, &OdStyle)> = Vec::new();
        let mut cur = self.styles.get_key_value(&(fam, name.to_string()));
        while let Some(((_, k), d)) = cur {
            if out.len() >= MAX_CHAIN || out.iter().any(|(o, _)| *o == k.as_str()) {
                break;
            }
            out.push((k.as_str(), d));
            cur = d
                .parent
                .as_deref()
                .and_then(|p| self.styles.get_key_value(&(fam, p.to_string())));
        }
        out
    }

    fn para(&self, name: Option<&str>) -> OdPara {
        let Some(name) = name else {
            return OdPara::default();
        };
        let chain = self.chain(Fam::Para, name);
        let mut fmt = Fmt::default();
        let mut hidden = None;
        for (_, d) in chain.iter().rev() {
            fmt = fmt.over(d.fmt);
            hidden = d.hidden.or(hidden);
        }
        let heading = chain
            .iter()
            .find_map(|(_, d)| d.outline)
            .map(|o| o.min(9))
            .or_else(|| {
                chain.iter().find_map(|(k, d)| {
                    heading_from_name(&d.display).or_else(|| heading_from_name(&decode_name(k)))
                })
            })
            .or_else(|| heading_from_name(&decode_name(name)));
        let code = chain
            .iter()
            .any(|(k, d)| is_code_name(&d.display) || is_code_name(&decode_name(k)));
        OdPara {
            heading,
            fmt,
            code,
            hidden: hidden.unwrap_or(false),
        }
    }

    fn char_style(&self, name: &str) -> (Fmt, bool) {
        let mut fmt = Fmt::default();
        let mut hidden = None;
        for (_, d) in self.chain(Fam::Text, name).iter().rev() {
            fmt = fmt.over(d.fmt);
            hidden = d.hidden.or(hidden);
        }
        (fmt, hidden.unwrap_or(false))
    }

    /// The level `lvl` (0-based) of list style `style`.
    fn level(&self, style: Option<&str>, lvl: usize) -> Option<&LevelDef> {
        self.lists.get(style?)?.get(lvl)?.as_ref()
    }

    fn read(&mut self, src: impl BufRead) -> Result<(), OfficeError> {
        let mut rd = XmlReader::new(src);
        let mut buf = Vec::new();
        let mut root = false;
        loop {
            buf.clear();
            let ev = rd.read_event_into(&mut buf).map_err(xml_err)?;
            let (e, empty) = match ev {
                Event::Start(e) => (e.into_owned(), false),
                Event::Empty(e) => (e.into_owned(), true),
                Event::Eof => return Ok(()),
                _ => continue,
            };
            if !root {
                root = true;
                continue;
            }
            match e.local_name().as_ref() {
                b"styles" | b"automatic-styles" => {
                    let mut budget = Budget::odf(500_000, 16 * 1024 * 1024);
                    if let Tree::Ok(node) = read_element(&mut rd, &e, empty, &mut budget)? {
                        self.add(&node);
                    }
                }
                _ => {
                    if !empty {
                        skip_rest(&mut rd)?;
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// numbers
// ---------------------------------------------------------------------------------------------

/// `a`, `b`, ..., `z`, `aa`, `ab` ... (spreadsheet columns; `style:num-letter-sync="false"`).
fn alphabetic(n: u32, base: u8) -> String {
    if n == 0 {
        return "0".into();
    }
    let mut n = n;
    let mut s = Vec::new();
    while n > 0 && s.len() < 8 {
        n -= 1;
        s.push((base + (n % 26) as u8) as char);
        n /= 26;
    }
    s.iter().rev().collect()
}

/// Katakana to hiragana (`ア` -> `あ`), for the `あ, い, う` format.
fn to_hiragana(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '\u{30A1}'..='\u{30F6}' => char::from_u32(c as u32 - 0x60).unwrap_or(c),
            c => c,
        })
        .collect()
}

fn cycle(alphabet: &str, n: u32) -> String {
    let cs: Vec<char> = alphabet.chars().collect();
    if n == 0 || cs.is_empty() {
        return n.to_string();
    }
    cs[((n - 1) as usize) % cs.len()].to_string()
}

/// A counter in an ODF `style:num-format`. LibreOffice writes a sample of the sequence for formats
/// beyond `1 a A i I` (`ア, イ, ウ, ...`): the first element names it.
fn format_od(raw: &str, sync: bool, n: u32) -> String {
    let n = n.min(999_999_999);
    let first = raw.split(',').next().unwrap_or("").trim();
    match first {
        "" => String::new(),
        "1" => n.to_string(),
        "01" => format_number("decimalZero", n),
        "a" if sync => format_number("lowerLetter", n),
        "A" if sync => format_number("upperLetter", n),
        "a" => alphabetic(n, b'a'),
        "A" => alphabetic(n, b'A'),
        "i" => format_number("lowerRoman", n),
        "I" => format_number("upperRoman", n),
        "１" => format_number("decimalFullWidth", n),
        "①" => format_number("decimalEnclosedCircle", n),
        "一" => format_number("japaneseCounting", n),
        "壱" => format_number("japaneseLegal", n),
        "ア" | "ｱ" => format_number("aiueo", n),
        "イ" | "ｲ" => format_number("iroha", n),
        "あ" => to_hiragana(&format_number("aiueo", n)),
        "い" => to_hiragana(&format_number("iroha", n)),
        "甲" => cycle("甲乙丙丁戊己庚辛壬癸", n),
        "子" => cycle("子丑寅卯辰巳午未申酉戌亥", n),
        _ => n.to_string(),
    }
}

/// Whether the format counts in plain decimal digits (`1`, and any format konoma does not know,
/// which is shown as decimal): the one a Markdown list can show.
fn is_decimal(raw: &str) -> bool {
    format_od(raw, false, 12_345) == "12345"
}

/// The label of an item: counters of the levels above and the item's own, as the document shows
/// them. `defs(k)` is the definition of level `k` (0-based), `counter(k)` its current value.
fn build_label(
    def: &LevelDef,
    li: usize,
    defs: &dyn Fn(usize) -> Option<LevelDef>,
    counter: &dyn Fn(usize) -> Option<i64>,
) -> String {
    let value = |k: usize| -> String {
        let d = defs(k);
        let (fmt, sync, start) = match &d {
            Some(d) => (d.format.as_str(), d.letter_sync, d.start),
            None => ("1", false, 1),
        };
        let v = counter(k).unwrap_or(start).max(0) as u32;
        format_od(fmt, sync, v)
    };
    if let Some(f) = &def.list_format {
        // `%1%.`: each `%k%` is the counter of level k.
        let mut out = String::new();
        let cs: Vec<char> = f.chars().collect();
        let mut i = 0;
        while i < cs.len() {
            if cs[i] == '%' {
                let mut j = i + 1;
                while j < cs.len() && cs[j].is_ascii_digit() && j - i <= 3 {
                    j += 1;
                }
                if j < cs.len() && cs[j] == '%' && j > i + 1 {
                    let k: usize = cs[i + 1..j].iter().collect::<String>().parse().unwrap_or(0);
                    if (1..=10).contains(&k) {
                        out.push_str(&value(k - 1));
                        i = j + 1;
                        continue;
                    }
                }
            }
            out.push(cs[i]);
            i += 1;
        }
        return out;
    }
    let from = (li + 1).saturating_sub(def.display_levels);
    let nums: Vec<String> = (from..=li).map(value).collect();
    format!("{}{}{}", def.prefix, nums.join("."), def.suffix)
}

// ---------------------------------------------------------------------------------------------
// the walk
// ---------------------------------------------------------------------------------------------

/// Where the list item being read is: the list instance (counters), its style and its depth.
#[derive(Debug, Clone)]
struct ListCtx {
    inst: usize,
    style: Option<String>,
    level: usize,
}

/// Per-paragraph reading state.
#[derive(Default)]
struct Ps {
    /// The text so far ends in a collapsible space (or the paragraph has just begun).
    prev_ws: bool,
    /// Formulas of the paragraph: position in `segs` and LaTeX.
    math: Vec<(usize, String)>,
    /// Something other than whitespace and formulas was read.
    other: bool,
}

struct Od<'a> {
    c: Conv<'a>,
    st: OdStyles,
    /// Counters of each list instance, per level.
    lists: Vec<[Option<i64>; 10]>,
    list_ids: HashMap<String, usize>,
    last_by_style: HashMap<String, usize>,
    outline: [Option<i64>; 10],
    /// `text:change-id`s of deletion regions.
    del_ids: HashSet<String>,
    /// Bytes of the table being read (cell text + separators, repeats counted), against the
    /// output budget: a repeat attribute makes a few bytes of XML stand for many copies.
    tbl_bytes: usize,
    /// Depth of deleted ranges being read (text is dropped while > 0).
    hidden: usize,
    defs: Vec<(usize, String)>,
    math_objects: usize,
    note_counter: i64,
}

/// All the text under `n` (OpenDocument keeps the text of every element), bounded.
fn node_text(n: &Node, out: &mut String, depth: usize) {
    if depth > 32 || out.len() > 4096 {
        return;
    }
    for k in &n.kids {
        match k {
            Kid::T(t) => out.push_str(t),
            Kid::N(c) => node_text(c, out, depth + 1),
        }
    }
}

/// `Pictures/x.png`, `./Pictures/x.png` -> the part name.
fn part_of(href: &str) -> Option<String> {
    let h = href.trim();
    if h.is_empty() || h.contains("://") || h.starts_with('/') || h.starts_with("..") {
        return None;
    }
    let h = h.trim_start_matches("./");
    if h.split('/').any(|s| s == "..") {
        return None;
    }
    Some(h.to_string())
}

const BLOCK_CONTAINERS: &[&str] = &[
    "section",
    "index-body",
    "index-title",
    "table-of-content",
    "alphabetical-index",
    "illustration-index",
    "table-index",
    "object-index",
    "user-index",
    "bibliography",
];

const SHAPES: &[&str] = &[
    "custom-shape",
    "rect",
    "ellipse",
    "circle",
    "polygon",
    "polyline",
    "path",
    "line",
    "connector",
    "caption",
    "measure",
    "regular-polygon",
    "g",
    "a-shape",
];

/// What an internal link's `#name|kind` suffix may be (LibreOffice's cross references).
const LINK_KINDS: &[&str] = &[
    "outline", "table", "text", "frame", "region", "graphic", "ole", "drawing", "sequence",
];

pub(super) fn convert(
    path: &Path,
    opts: &DocOptions,
    cancel: Option<&Cancel>,
) -> Result<Document, OfficeError> {
    let cap = opts.limits.max_part_bytes;
    let mut pkg = Pkg::open(path)?;
    // The package must say it is an OpenDocument *text* (a spreadsheet or a presentation is not ours).
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
        .starts_with(b"application/vnd.oasis.opendocument.text")
    {
        return Err(OfficeError::Unsupported);
    }

    // A damaged styles part costs the document its styles, not the document.
    let mut st = OdStyles::default();
    if let Some(r) = pkg.part("styles.xml", cap)? {
        let _ = st.read(r);
    }

    let media = Pkg::open(path)?;
    let conv = Conv::new(
        opts,
        cancel,
        Styles::default(),
        Numbering::default(),
        HashMap::new(),
        media,
    );
    let mut od = Od {
        c: conv,
        st,
        lists: Vec::new(),
        list_ids: HashMap::new(),
        last_by_style: HashMap::new(),
        outline: [None; 10],
        del_ids: HashSet::new(),
        tbl_bytes: 0,
        hidden: 0,
        defs: Vec::new(),
        math_objects: 0,
        note_counter: 0,
    };
    {
        let Some(r) = pkg.part("content.xml", cap)? else {
            return Err(OfficeError::Corrupt("missing content.xml".into()));
        };
        od.read_content(r)?;
    }
    od.c.flush_carry_top();
    od.c.flush_code();
    let Od { c, defs, .. } = od;
    Ok(c.assemble(defs))
}

impl<'a> Od<'a> {
    fn cancelled(&self) -> bool {
        self.c.cancelled()
    }

    // -----------------------------------------------------------------------------------------
    // content.xml
    // -----------------------------------------------------------------------------------------

    fn read_content(&mut self, src: impl BufRead) -> Result<(), OfficeError> {
        let mut rd = XmlReader::new(src);
        let mut buf = Vec::new();
        let mut root = false;
        let mut in_body = false;
        // The automatic styles come before the body; the body's first child is the document type.
        loop {
            buf.clear();
            let ev = rd.read_event_into(&mut buf).map_err(xml_err)?;
            let (e, empty) = match ev {
                Event::Start(e) => (e.into_owned(), false),
                Event::Empty(e) => (e.into_owned(), true),
                Event::Eof => return Ok(()),
                _ => continue,
            };
            let name = String::from_utf8_lossy(e.local_name().as_ref()).into_owned();
            if !root {
                root = true;
                if name != "document-content" {
                    return Err(OfficeError::Unsupported);
                }
                continue;
            }
            if in_body {
                if name != "text" {
                    return Err(OfficeError::Unsupported);
                }
                if empty {
                    return Ok(());
                }
                break;
            }
            match name.as_str() {
                "body" => in_body = !empty,
                "automatic-styles" => {
                    let mut budget = Budget::odf(500_000, 16 * 1024 * 1024);
                    if let Tree::Ok(node) = read_element(&mut rd, &e, empty, &mut budget)? {
                        self.st.add(&node);
                    }
                }
                _ => {
                    if !empty {
                        skip_rest(&mut rd)?;
                    }
                }
            }
        }
        // Inside `office:text`: its children are the top-level blocks. Sections and indexes are
        // entered (their children are top-level blocks too), so a document wrapped in one section
        // is still read one block at a time.
        let mut entered = 0usize;
        let mut count = 0usize;
        loop {
            buf.clear();
            let ev = rd.read_event_into(&mut buf).map_err(xml_err)?;
            let (e, empty) = match ev {
                Event::Start(e) => (e.into_owned(), false),
                Event::Empty(e) => (e.into_owned(), true),
                Event::End(_) => {
                    if entered == 0 {
                        return Ok(());
                    }
                    entered -= 1;
                    continue;
                }
                Event::Eof => {
                    return Err(OfficeError::Corrupt(
                        "xml: unexpected end of document".into(),
                    ))
                }
                _ => continue,
            };
            count += 1;
            if count > self.c.opts.max_blocks || self.cancelled() || self.c.full {
                self.c.truncated = true;
                return Ok(());
            }
            let name = String::from_utf8_lossy(e.local_name().as_ref()).into_owned();
            let hidden_section =
                name == "section" && attr(&e, b"display", true).is_some_and(|v| v.trim() == "none");
            if !empty && BLOCK_CONTAINERS.contains(&name.as_str()) && !hidden_section {
                entered += 1;
                continue;
            }
            let mut budget = Budget::odf(
                self.c.opts.max_block_nodes,
                self.c.opts.max_block_text_bytes,
            );
            match read_element(&mut rd, &e, empty, &mut budget)? {
                Tree::Ok(node) => {
                    if hidden_section {
                        // Not shown, but a deletion range may end in it.
                        self.skip(&node, 0);
                        continue;
                    }
                    let mut blks = Vec::new();
                    self.block(&node, Ctx::Body, 0, &mut blks);
                    self.c.write_blocks(blks);
                }
                Tree::TooBig => {
                    self.c.truncated = true;
                    return Ok(());
                }
            }
            if self.c.full {
                self.c.truncated = true;
                return Ok(());
            }
        }
    }

    // -----------------------------------------------------------------------------------------
    // blocks
    // -----------------------------------------------------------------------------------------

    fn blocks(&mut self, kids: &[Kid], ctx: Ctx, depth: usize, out: &mut Vec<Blk>) {
        if depth > 40 {
            return;
        }
        for k in kids {
            if let Kid::N(n) = k {
                self.block(n, ctx, depth, out);
            }
        }
    }

    fn block(&mut self, n: &Node, ctx: Ctx, depth: usize, out: &mut Vec<Blk>) {
        if depth > 40 {
            return;
        }
        match n.name.as_str() {
            "p" | "h" => self.para(n, ctx, None, depth, out),
            "list" => self.list(n, ctx, None, depth, out),
            "table" => {
                self.c.flush_carry(out, ctx);
                if let Some(t) = self.table(n, depth) {
                    out.push(t);
                }
            }
            "tracked-changes" => self.tracked_changes(n),
            "change-start" | "change-end" | "change" => self.change(n),
            name if BLOCK_CONTAINERS.contains(&name) => {
                if n.attr("display").is_some_and(|v| v.trim() == "none") && name == "section" {
                    self.skip(n, depth);
                    return;
                }
                self.blocks(&n.kids, ctx, depth + 1, out)
            }
            "frame" => {
                let mut tmp = Inl::default();
                let mut ps = Ps::default();
                self.frame(n, ctx, &mut tmp, &mut ps, depth);
                self.flush_inline(tmp, ctx, out);
            }
            name if SHAPES.contains(&name) => self.shape(n, ctx, depth, out),
            _ => {}
        }
    }

    /// Turns what an inline walk collected outside a paragraph (a shape's picture) into blocks.
    fn flush_inline(&mut self, inl: Inl, ctx: Ctx, out: &mut Vec<Blk>) {
        let Inl { segs, extras, .. } = inl;
        if !segs.is_empty() {
            let role = Role {
                heading: None,
                code: false,
                list: None,
            };
            let mut b = self.c.make_blocks(segs, &role, Vec::new(), ctx);
            out.append(&mut b);
        }
        out.extend(extras);
    }

    /// The paragraphs of a drawing shape (a custom shape, a group ..), in document order.
    fn shape(&mut self, n: &Node, ctx: Ctx, depth: usize, out: &mut Vec<Blk>) {
        if depth > 30 {
            return;
        }
        if self.hidden > 0 {
            self.skip(n, depth);
            return;
        }
        for k in n.nodes() {
            match k.name.as_str() {
                "p" | "h" | "list" | "table" | "frame" => self.block(k, ctx, depth + 1, out),
                name if SHAPES.contains(&name) => self.shape(k, ctx, depth + 1, out),
                _ => {}
            }
        }
    }

    fn tracked_changes(&mut self, n: &Node) {
        for region in n.nodes() {
            if region.name == "changed-region" && region.child("deletion").is_some() {
                if let Some(id) = region.attr("id") {
                    if self.del_ids.len() < 100_000 {
                        self.del_ids.insert(id.to_string());
                    }
                }
            }
        }
    }

    /// A subtree that is not shown (a hidden paragraph or section, anything inside a deletion):
    /// its text is dropped, but the marks of a deletion range are still read -- a `change-end`
    /// in it would otherwise leave every later paragraph hidden.
    fn skip(&mut self, n: &Node, depth: usize) {
        if depth > 60 {
            return;
        }
        for k in n.nodes() {
            match k.name.as_str() {
                "change-start" | "change-end" => self.change(k),
                _ => self.skip(k, depth + 1),
            }
        }
    }

    /// `text:change-start` / `text:change-end`: the range of a deletion is not shown.
    fn change(&mut self, n: &Node) {
        let Some(id) = n.attr("change-id") else {
            return;
        };
        if !self.del_ids.contains(id) {
            return;
        }
        match n.name.as_str() {
            "change-start" => self.hidden += 1,
            "change-end" => self.hidden = self.hidden.saturating_sub(1),
            _ => {}
        }
    }

    // -----------------------------------------------------------------------------------------
    // tables
    // -----------------------------------------------------------------------------------------

    fn table(&mut self, t: &Node, depth: usize) -> Option<Blk> {
        if depth > 12 {
            return None;
        }
        let mut rows: Vec<Vec<String>> = Vec::new();
        let mut cells = 0usize;
        // Each table has its own byte count; an enclosing table sees a nested one as cell text.
        let outer_bytes = std::mem::take(&mut self.tbl_bytes);
        self.table_rows(t, &mut rows, &mut cells, depth, 0);
        self.tbl_bytes = outer_bytes;
        // A table inside a deleted range has its cells read (the range may end in one) but empty.
        let blank = rows.iter().all(|r| r.iter().all(String::is_empty));
        (!rows.is_empty() && !(blank && self.hidden > 0)).then_some(Blk::Table(rows))
    }

    /// Returns false once the cell budget is spent.
    fn table_rows(
        &mut self,
        n: &Node,
        rows: &mut Vec<Vec<String>>,
        cells: &mut usize,
        depth: usize,
        rdepth: usize,
    ) -> bool {
        if rdepth > 8 {
            return true;
        }
        for c in n.nodes() {
            match c.name.as_str() {
                "table-row" => {
                    if matches!(c.attr("visibility"), Some("collapse" | "filter")) {
                        continue;
                    }
                    let rep = c
                        .attr("number-rows-repeated")
                        .and_then(|v| v.trim().parse::<usize>().ok())
                        .unwrap_or(1)
                        .clamp(1, 10_000);
                    let mut row: Vec<String> = Vec::new();
                    let mut row_bytes = 0usize;
                    let mut row_over = false;
                    for cell in c.nodes() {
                        let covered = match cell.name.as_str() {
                            "table-cell" => false,
                            "covered-table-cell" => true,
                            _ => continue,
                        };
                        let crep = cell
                            .attr("number-columns-repeated")
                            .and_then(|v| v.trim().parse::<usize>().ok())
                            .unwrap_or(1)
                            .clamp(1, 1_024);
                        let text = if covered {
                            String::new()
                        } else {
                            let mut blks = Vec::new();
                            self.blocks(&cell.kids, Ctx::Cell, depth + 1, &mut blks);
                            self.c.flush_carry(&mut blks, Ctx::Cell);
                            cell_text(&blks, "<br>")
                        };
                        // The copies are made only once they are known to fit the output budget
                        // (a repeat count is a few bytes of XML standing for that many cells).
                        let room = self.c.body_bytes.saturating_sub(self.tbl_bytes + row_bytes);
                        let each = text.len() + 3;
                        let fit = crep.min(room / each);
                        for _ in 0..fit {
                            row.push(text.clone());
                        }
                        row_bytes += each * fit;
                        held(self.tbl_bytes + row_bytes);
                        if fit < crep {
                            self.c.truncated = true;
                            row_over = true;
                            break;
                        }
                        if row.len() > 16_384 {
                            break;
                        }
                    }
                    // A run of empty repeated rows (a sheet-like table padded to its size) is one row.
                    let mut rep = if row.iter().all(String::is_empty) {
                        1
                    } else {
                        rep
                    };
                    let wanted = rep;
                    // As many repeats as the budget has room for (a row that was cut is kept once).
                    let room = self.c.body_bytes.saturating_sub(self.tbl_bytes);
                    if let Some(fit) = room.checked_div(row_bytes) {
                        rep = rep.min(fit);
                    }
                    if row_over {
                        rep = rep.min(1);
                    }
                    let over = row_over || rep < wanted;
                    if over {
                        self.c.truncated = true;
                    }
                    *cells += row.len() * rep;
                    if *cells > self.c.opts.max_table_cells {
                        self.c.truncated = true;
                        return false;
                    }
                    self.tbl_bytes += row_bytes.saturating_mul(rep);
                    if !row.is_empty() {
                        for _ in 0..rep {
                            rows.push(row.clone());
                        }
                    }
                    held(self.tbl_bytes);
                    if over {
                        return false;
                    }
                }
                "table-header-rows" | "table-rows" | "table-row-group" => {
                    let more = self.table_rows(c, rows, cells, depth, rdepth + 1);
                    if !more {
                        return false;
                    }
                }
                _ => {}
            }
        }
        true
    }

    // -----------------------------------------------------------------------------------------
    // lists
    // -----------------------------------------------------------------------------------------

    fn enter_list(&mut self, n: &Node, parent: Option<&ListCtx>) -> ListCtx {
        let own = n.attr("style-name").map(str::to_string);
        if let Some(p) = parent {
            return ListCtx {
                inst: p.inst,
                style: own.or_else(|| p.style.clone()),
                level: p.level + 1,
            };
        }
        let mut inst = None;
        if let Some(id) = n.attr("continue-list") {
            inst = self.list_ids.get(id).copied();
        }
        if inst.is_none()
            && n.attr("continue-numbering")
                .is_some_and(|v| v.trim() == "true")
        {
            if let Some(s) = &own {
                inst = self.last_by_style.get(s).copied();
            }
        }
        let inst = match inst {
            Some(i) => i,
            None => {
                self.lists.push([None; 10]);
                self.lists.len() - 1
            }
        };
        if let Some(id) = n.attr("id") {
            if self.list_ids.len() < 100_000 {
                self.list_ids.insert(id.to_string(), inst);
            }
        }
        if let Some(s) = &own {
            if self.last_by_style.len() < 100_000 {
                self.last_by_style.insert(s.clone(), inst);
            }
        }
        ListCtx {
            inst,
            style: own,
            level: 0,
        }
    }

    fn list(
        &mut self,
        n: &Node,
        ctx: Ctx,
        parent: Option<&ListCtx>,
        depth: usize,
        out: &mut Vec<Blk>,
    ) {
        if depth > 40 {
            return;
        }
        if self.lists.len() > 100_000 {
            self.c.truncated = true;
            return;
        }
        let lc = self.enter_list(n, parent);
        for item in n.nodes() {
            match item.name.as_str() {
                "list-item" => self.list_item(item, &lc, ctx, depth, out),
                // A header of a list is text that is not numbered.
                "list-header" => self.blocks(&item.kids, ctx, depth + 1, out),
                _ => {}
            }
        }
    }

    fn list_item(&mut self, item: &Node, lc: &ListCtx, ctx: Ctx, depth: usize, out: &mut Vec<Blk>) {
        // The item whose first paragraph carries the number; later paragraphs continue it.
        let mut first: Option<usize> = None;
        let mut counted = false;
        for k in item.nodes() {
            match k.name.as_str() {
                "p" | "h" => {
                    if !counted {
                        counted = true;
                        let role = self.number(lc, item.attr("start-value"));
                        let before = out.len();
                        self.para(k, ctx, role, depth + 1, out);
                        first = out[before..]
                            .iter()
                            .position(|b| matches!(b, Blk::Item(_)))
                            .map(|i| i + before);
                    } else {
                        let mut tmp = Vec::new();
                        self.para(k, ctx, None, depth + 1, &mut tmp);
                        for b in tmp {
                            match (b, first) {
                                (Blk::Para(t), Some(i)) => {
                                    if let Some(Blk::Item(it)) = out.get_mut(i) {
                                        it.text.push('\n');
                                        it.text.push_str(&t);
                                    }
                                }
                                (b, _) => out.push(b),
                            }
                        }
                    }
                }
                "list" => self.list(k, ctx, Some(lc), depth + 1, out),
                _ => self.block(k, ctx, depth + 1, out),
            }
        }
        // An item that starts with a nested list still takes its number.
        if !counted {
            let _ = self.number(lc, item.attr("start-value"));
        }
    }

    /// Advances the counters for one item and returns how it is shown (`None`: as a plain paragraph).
    fn number(&mut self, lc: &ListCtx, start_value: Option<&str>) -> Option<ListRole> {
        let li = lc.level.min(9);
        let def = self.st.level(lc.style.as_deref(), li).cloned();
        let counters = &mut self.lists[lc.inst];
        let start = def.as_ref().map_or(1, |d| d.start);
        let value = match start_value.and_then(|v| v.trim().parse::<i64>().ok()) {
            Some(v) => v.clamp(0, 999_999_999),
            None => match counters[li] {
                None => start,
                Some(v) => (v + 1).min(999_999_999),
            },
        };
        counters[li] = Some(value);
        for c in counters.iter_mut().skip(li + 1) {
            *c = None;
        }
        let snapshot = *counters;
        let level = lc.level.min(8) as u8;
        let Some(def) = def.filter(|d| d.number) else {
            // A bullet or image level, or no list style: a bullet.
            return Some(ListRole {
                level,
                list: lc.inst as u32,
                marker: Marker::Bullet,
                label: String::new(),
            });
        };
        let style = lc.style.clone();
        let this = &*self;
        let label = build_label(
            &def,
            li,
            &|k| this.st.level(style.as_deref(), k).cloned(),
            &|k| snapshot[k],
        );
        let label = clean(&label);
        if label.trim().is_empty() {
            return None;
        }
        let real = is_decimal(&def.format)
            && def
                .list_format
                .as_deref()
                .is_none_or(|f| f == format!("%{}%.", li + 1))
            && label == format!("{value}.");
        Some(ListRole {
            level,
            list: lc.inst as u32,
            marker: if real {
                Marker::Ordered(value as u32)
            } else {
                Marker::Literal
            },
            label,
        })
    }

    /// The outline numbering label of a heading (`text:outline-style`), if the document numbers
    /// its headings.
    fn heading_label(&mut self, level: u8, h: &Node) -> Option<ListRole> {
        let li = usize::from(level.clamp(1, 10)) - 1;
        let def = self.st.outline.get(li).cloned().flatten()?;
        if !def.number || (def.format.trim().is_empty() && def.list_format.is_none()) {
            return None;
        }
        if h.attr("is-list-header").is_some_and(|v| v.trim() == "true") {
            return None;
        }
        let restart = h
            .attr("restart-numbering")
            .is_some_and(|v| v.trim() == "true");
        let value = match (
            restart,
            h.attr("start-value")
                .and_then(|v| v.trim().parse::<i64>().ok()),
        ) {
            (true, Some(v)) => v.clamp(0, 999_999_999),
            _ => match self.outline[li] {
                None => def.start,
                Some(v) => (v + 1).min(999_999_999),
            },
        };
        self.outline[li] = Some(value);
        for c in self.outline.iter_mut().skip(li + 1) {
            *c = None;
        }
        let snapshot = self.outline;
        let this = &*self;
        let label = clean(&build_label(
            &def,
            li,
            &|k| this.st.outline.get(k).cloned().flatten(),
            &|k| snapshot[k],
        ));
        if label.trim().is_empty() {
            return None;
        }
        Some(ListRole {
            level: 0,
            list: 0,
            marker: Marker::Literal,
            label,
        })
    }

    // -----------------------------------------------------------------------------------------
    // paragraphs
    // -----------------------------------------------------------------------------------------

    fn para(
        &mut self,
        p: &Node,
        ctx: Ctx,
        list: Option<ListRole>,
        depth: usize,
        out: &mut Vec<Blk>,
    ) {
        // (A paragraph inside a deleted range still has to be read: the range may end in it.)
        if depth > 60 {
            return;
        }
        let ps_style = self.st.para(p.attr("style-name"));
        if ps_style.hidden {
            // Not shown, but a deletion range may start or end inside it.
            self.skip(p, depth);
            return;
        }
        let heading = if p.name == "h" {
            let lvl = p
                .attr("outline-level")
                .and_then(|v| v.trim().parse::<u8>().ok())
                .or(ps_style.heading)
                .unwrap_or(1);
            Some(lvl.clamp(1, 9))
        } else {
            ps_style.heading
        };
        let mut base = ps_style.fmt;
        if heading.is_some() {
            base.bold = None;
            base.italic = None;
        }
        // The text of a paragraph whose end was inside a deleted range (the break between the two
        // paragraphs was deleted) goes on in this one.
        let carried = std::mem::take(&mut self.c.carry);
        let mut ps = Ps {
            prev_ws: true,
            other: carried.iter().any(|g| match g {
                Seg::Text(t, _) => t.chars().any(|c| !c.is_whitespace()),
                Seg::Raw(_) | Seg::Math(_) | Seg::Display(_) => true,
                _ => false,
            }),
            ..Ps::default()
        };
        let mut inl = Inl {
            segs: carried,
            ..Inl::default()
        };
        self.inline_kids(p, base, ctx, &mut inl, &mut ps, depth);
        if self.hidden > 0 && inl.extras.is_empty() {
            // The deletion range runs past this paragraph's end: its break is deleted.
            self.c.carry = inl.segs;
            return;
        }
        let Inl {
            mut segs,
            extras,
            mut bookmarks,
        } = inl;
        // A heading is also what a LibreOffice cross reference `#Text|outline` names.
        if p.name == "h" && bookmarks.len() < 64 {
            let text = outline_text(&segs);
            if !text.trim().is_empty() {
                bookmarks.push(format!("{}|outline", text.trim()));
            }
        }
        // A heading takes the number of its list, or else of the outline numbering.
        let list = match (heading, list) {
            (Some(_), Some(l)) => Some(l),
            (Some(level), None) if p.name == "h" => self.heading_label(level, p),
            (_, l) => l,
        };
        // A paragraph that is one formula is a display formula (not a heading or a list item: those
        // keep the formula in their line).
        if ps.math.len() == 1 && !ps.other && heading.is_none() && list.is_none() {
            if let Some((i, latex)) = ps.math.pop() {
                if i < segs.len() {
                    segs[i] = Seg::Display(Ok(latex));
                }
            }
        }
        let role = Role {
            heading,
            code: ps_style.code && heading.is_none() && list.is_none(),
            list,
        };
        let mut blks = self.c.make_blocks(segs, &role, bookmarks, ctx);
        out.append(&mut blks);
        out.extend(extras);
    }

    // -----------------------------------------------------------------------------------------
    // inline content
    // -----------------------------------------------------------------------------------------

    fn inline_kids(
        &mut self,
        n: &Node,
        base: Fmt,
        ctx: Ctx,
        inl: &mut Inl,
        ps: &mut Ps,
        depth: usize,
    ) {
        if depth > INLINE_DEPTH {
            return;
        }
        for k in &n.kids {
            match k {
                Kid::T(t) => self.text(t, base, inl, ps),
                Kid::N(c) => self.inline_node(c, base, ctx, inl, ps, depth),
            }
        }
    }

    /// Text of the XML: runs of white space are one space, and a paragraph does not start with one.
    fn text(&mut self, t: &str, fmt: Fmt, inl: &mut Inl, ps: &mut Ps) {
        if self.hidden > 0 {
            return;
        }
        let mut s = String::with_capacity(t.len());
        for ch in t.chars() {
            if matches!(ch, ' ' | '\t' | '\n' | '\r') {
                if !ps.prev_ws {
                    s.push(' ');
                    ps.prev_ws = true;
                }
            } else {
                s.push(ch);
                ps.prev_ws = false;
            }
        }
        let s = clean(&s);
        if s.chars().any(|c| !c.is_whitespace()) {
            ps.other = true;
        }
        self.c.push_text(inl, &s, fmt);
    }

    fn inline_node(
        &mut self,
        n: &Node,
        base: Fmt,
        ctx: Ctx,
        inl: &mut Inl,
        ps: &mut Ps,
        depth: usize,
    ) {
        if depth > INLINE_DEPTH {
            return;
        }
        match n.name.as_str() {
            "span" => {
                let (f, hidden) = match n.attr("style-name") {
                    Some(s) => self.st.char_style(s),
                    None => (Fmt::default(), false),
                };
                if hidden {
                    self.skip(n, depth);
                } else {
                    self.inline_kids(n, base.over(f), ctx, inl, ps, depth + 1);
                }
            }
            "a" => {
                let url = self.link_dest(n);
                inl.segs.push(Seg::LinkOpen(url));
                self.inline_kids(n, base, ctx, inl, ps, depth + 1);
                inl.segs.push(Seg::LinkClose);
            }
            "s" => {
                if self.hidden == 0 {
                    let count = n
                        .attr("c")
                        .and_then(|v| v.trim().parse::<usize>().ok())
                        .unwrap_or(1)
                        .clamp(1, 256);
                    self.c.push_text(inl, &" ".repeat(count), base);
                    ps.prev_ws = false;
                }
            }
            "tab" => {
                if self.hidden == 0 {
                    let tab: String = std::iter::repeat_n(NBSP, 4).collect();
                    self.c.push_text(inl, &tab, Fmt::default());
                    ps.prev_ws = false;
                }
            }
            "line-break" => {
                if self.hidden == 0 {
                    inl.segs.push(Seg::Break);
                    ps.prev_ws = false;
                }
            }
            "bookmark" | "bookmark-start" => {
                if let Some(name) = n.attr("name") {
                    if inl.bookmarks.len() < 64 {
                        inl.bookmarks.push(name.to_string());
                    }
                }
            }
            // Content that is not a character ends a run of white space: `a <frame/> b` keeps both.
            "note" => {
                self.note(n, ctx, inl, ps, depth);
                ps.prev_ws = false;
            }
            "frame" => {
                self.frame(n, ctx, inl, ps, depth);
                ps.prev_ws = false;
            }
            name if SHAPES.contains(&name) => {
                let mut blks = Vec::new();
                self.shape(n, ctx, depth + 8, &mut blks);
                inl.extras.append(&mut blks);
                ps.prev_ws = false;
            }
            "change-start" | "change-end" | "change" => self.change(n),
            "ruby" => {
                let start = inl.segs.len();
                let mut reading = String::new();
                for k in n.nodes() {
                    if k.name == "ruby-base" {
                        self.inline_kids(k, base, ctx, inl, ps, depth + 1);
                    } else if k.name == "ruby-text" && self.hidden == 0 {
                        node_text(k, &mut reading, 0);
                    }
                }
                let last = inl.segs[start..].iter().rev().find_map(|g| match g {
                    Seg::Text(t, _) => t.chars().next_back(),
                    _ => None,
                });
                self.c.push_reading(inl, &reading, last, base);
            }
            // Text that is not shown (its deletion marks still count).
            "hidden-text" | "hidden-paragraph" => self.skip(n, depth),
            // Comments, the pronunciation of a ruby, a note's own label.
            "annotation" | "annotation-end" | "ruby-text" | "soft-page-break" | "bookmark-end"
            | "note-citation" => {}
            // `svg:title` / `svg:desc` describe a frame (read by it); `text:title` is a field.
            "title" | "desc" if n.prefix != "text" => {}
            // Fields and any other wrapper: their stored text.
            _ => self.inline_kids(n, base, ctx, inl, ps, depth + 1),
        }
    }

    fn link_dest(&mut self, n: &Node) -> String {
        let Some(href) = n.attr("href") else {
            return String::new();
        };
        if let Some(name) = href.strip_prefix('#') {
            // `#Heading text|outline` is a cross reference to a heading by its text: such a heading
            // registers `Heading text|outline` as one of its names (see `para`). The other kinds
            // (`|table`, `|frame` ..) name things Markdown has no anchor for: the bare name.
            let name = match name.rsplit_once('|') {
                Some((_, "outline")) => name,
                Some((a, kind)) if LINK_KINDS.contains(&kind) => a,
                _ => name,
            };
            return if name.is_empty() || name.starts_with('|') {
                String::new()
            } else {
                self.c.anchor_dest(name)
            };
        }
        safe_url(href).unwrap_or_default()
    }

    fn note(&mut self, n: &Node, ctx: Ctx, inl: &mut Inl, ps: &mut Ps, depth: usize) {
        let _ = ctx;
        if self.hidden > 0 || self.c.in_note {
            self.skip(n, depth);
            return;
        }
        let Some(body) = n.child("note-body") else {
            return;
        };
        if self.c.note_labels.len() >= self.c.opts.max_notes {
            self.c.truncated = true;
            return;
        }
        let is_end = n.attr("note-class") == Some("endnote");
        self.note_counter += 1;
        self.c.note_labels.push((is_end, self.note_counter));
        let pos = self.c.note_labels.len();
        // The body is read here, where it stands (an ODF note is inline); the Markdown definition
        // goes after the document.
        self.c.in_note = true;
        let mut blks = Vec::new();
        self.blocks(&body.kids, Ctx::Note, depth + 8, &mut blks);
        self.c.in_note = false;
        self.c.flush_carry(&mut blks, Ctx::Note);
        let mut text = note_text(&blks);
        if text.trim().is_empty() {
            text = "\u{2014}".into();
        }
        if self.c.keep_note(text.len()) {
            self.defs.push((pos, text));
        }
        inl.segs.push(Seg::Raw(format!("[^{pos}]")));
        ps.other = true;
    }

    // -----------------------------------------------------------------------------------------
    // frames: pictures, formulas, text boxes
    // -----------------------------------------------------------------------------------------

    fn frame(&mut self, f: &Node, ctx: Ctx, inl: &mut Inl, ps: &mut Ps, depth: usize) {
        if depth > 40 {
            return;
        }
        if self.hidden > 0 {
            self.skip(f, depth);
            return;
        }
        self.c.cur_ctx = ctx;
        let pick = |name: &str| -> String {
            f.child(name)
                .map(|n| n.text())
                .map(|t| clean(t.trim()))
                .unwrap_or_default()
        };
        let desc = pick("desc");
        let alt: String = if desc.trim().is_empty() {
            pick("title")
        } else {
            desc
        }
        .chars()
        .take(300)
        .collect();

        let mut shown = false;
        // A formula (LibreOffice Math): an embedded object whose content is MathML.
        for o in f.nodes().filter(|k| k.name == "object") {
            if let Some(done) = self.formula(o, inl, ps) {
                shown = done;
                if done {
                    break;
                }
            }
        }
        if !shown {
            if let Some(im) = f.nodes().find(|k| k.name == "image") {
                let md = match im.attr("href").and_then(part_of) {
                    Some(part) => self.c.image_md_part(&part, &alt),
                    None => self.c.placeholder(&alt),
                };
                inl.segs.push(Seg::Raw(md));
                ps.other = true;
                shown = true;
            }
        }
        if !shown
            && f.nodes()
                .any(|k| matches!(k.name.as_str(), "object" | "object-ole" | "plugin"))
            && f.child("text-box").is_none()
        {
            let md = self.c.placeholder(&alt);
            inl.segs.push(Seg::Raw(md));
            ps.other = true;
        }
        for tb in f.nodes().filter(|k| k.name == "text-box") {
            let mut blks = Vec::new();
            self.blocks(&tb.kids, ctx, depth + 8, &mut blks);
            inl.extras.append(&mut blks);
        }
    }

    /// Reads the formula object `o` points at. `Some(true)`: shown (LaTeX or the source text),
    /// `Some(false)`: an object that is not a formula, `None`: nothing to read.
    fn formula(&mut self, o: &Node, inl: &mut Inl, ps: &mut Ps) -> Option<bool> {
        let href = o.attr("href")?;
        let dir = part_of(href)?;
        if dir.len() > OBJECT_DIR_MAX {
            return None;
        }
        let dir = dir.trim_end_matches('/');
        if self.math_objects >= self.c.opts.max_math_objects {
            self.c.truncated = true;
            return Some(false);
        }
        let part = format!("{dir}/content.xml");
        let max = self.c.opts.max_math_xml;
        let mut bytes = Vec::new();
        {
            let r = self.c.media.part(&part, max as u64 + 1).ok()??;
            r.take(max as u64 + 1).read_to_end(&mut bytes).ok()?;
        }
        if bytes.len() > max {
            self.math_objects += 1;
            self.c.math_total += 1;
            let md = self.c.placeholder("formula");
            inl.segs.push(Seg::Raw(md));
            ps.other = true;
            return Some(true);
        }
        let xml = String::from_utf8_lossy(&bytes).into_owned();
        if !mathml::is_math(&xml) {
            return Some(false);
        }
        self.math_objects += 1;
        self.c.math_total += 1;
        let latex = if self.c.cancelled() {
            None
        } else {
            (self.c.opts.mathml)(&xml, false)
                .map(|l| omml::tidy(&l.replace(['\n', '\r'], " ")))
                .filter(|l| !l.is_empty() && !omml::has_bare_dollar(l) && !l.contains(NBSP))
        };
        match latex {
            Some(l) => {
                self.c.math_latex += 1;
                ps.math.push((inl.segs.len(), l.clone()));
                inl.segs.push(Seg::Math(l));
            }
            None => {
                // The StarMath source reads like the formula; failing that, its characters.
                let text = clean(&mathml::fallback_text(&xml));
                if !text.trim().is_empty() {
                    inl.segs.push(Seg::Text(text, F::default()));
                    ps.other = true;
                }
            }
        }
        Some(true)
    }
}
