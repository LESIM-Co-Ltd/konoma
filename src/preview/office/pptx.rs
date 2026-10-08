//! PowerPoint preview (pptx / pptm / ppsx / ppsm / potx / potm): the reading side. A presentation is
//! read by konoma itself and converted to a Markdown string, one `##` section per slide, that
//! konoma's own Markdown renderer draws (design: `docs/FEATURE-OFFICE-PREVIEW.md` section 11).
//!
//! This is a child module of `docx.rs`: the writer (escaping, lists, tables, pictures, charts,
//! math, the output budgets) is the Word converter's, and only what is particular to slides lives
//! here.
//!
//! # Shape of the Markdown
//!
//! ```text
//! ## Slide 1: Title of the slide
//!
//! Subtitle
//!
//! - bullet
//!   - nested bullet
//!
//! > **Notes**
//! > the speaker notes
//!
//! ## Slide 2 (hidden)
//! ```
//!
//! **Invariant** (the App jumps between slides by their headings): the Markdown holds one level-2
//! heading per slide, in the order of the slides, and no other level-2 heading. Nothing inside a
//! slide is ever a heading (the title is in the slide's heading; a text that looks like `## x` is
//! escaped). [`Document::slides`] lists the slides in the same order.
//!
//! | PowerPoint | Markdown |
//! |---|---|
//! | slide order (`p:sldIdLst` + relationships, never file names) | `## Slide N: title` |
//! | hidden slide (`show="0"`) | the heading ends with ` (hidden)` |
//! | title / centred title | the heading; other titles on the slide are text |
//! | text, in reading order ([`super::super::slide_order`]) | paragraphs |
//! | bullets / numbering (`a:buChar`, `a:buAutoNum`, `a:buNone`; inherited from the shape's list style, the layout, the master) | `- ` / `N. ` lists (other number formats: the label as text) |
//! | bold, italic, strike, `baseline` | `**` `*` `~~` `<sup>` `<sub>` |
//! | hyperlinks on runs (external only) | `[text](url)` |
//! | table | GFM table |
//! | picture | `![alt](office-img://..)` |
//! | chart | `[chart: title]` |
//! | embedded object with no picture (`p:oleObj`) | `[object: progId]` |
//! | slide number field (`a:fld type="slidenum"`) | the slide's number (`firstSlideNum` honoured) |
//! | `mc:AlternateContent` | the first `Choice` whose `Requires` namespaces are read (`a14`), else the `Fallback` (ink: its picture) |
//! | SmartArt | the text of its drawing (else of its data), as a list; else `[SmartArt]` |
//! | `a14:m` math | `$latex$` / `$$latex$$` |
//! | speaker notes (the notes page's body placeholder) | a quote block under the slide |
//! | connectors, lines, shapes without text, date / footer / slide number placeholders, comments | nothing |
//!
//! Positions are resolved through inheritance (a placeholder with no `a:xfrm` takes the layout's
//! placeholder of its `idx` -- or of its type when it has none -- then the master's; an `idx` the
//! layout lacks inherits nothing) and through group transforms (`a:chOff` / `a:chExt`); a rotated
//! shape is approximated by its bounding box.
//!
//! # Safety
//!
//! The package goes through `container` first. Every XML part is read through `XmlReader` (depth
//! 256) into at most [`DocOptions::max_slide_part_bytes`] bytes, all of them together at most
//! [`DocOptions::max_pptx_read_total`]; the tree of a slide is held under the node budget of
//! [`DocOptions`] and dropped when the slide is written; slides, shapes per slide, pictures, table
//! cells and the output have budgets, and going over any of them sets [`Document::truncated`].
//! Nothing here panics on any input (a panic inside is caught and reported as `Corrupt`).

use std::rc::Rc;

use super::super::docx_xml::skip_rest;
use super::*;
use crate::i18n::{tr, Msg};
use crate::preview::office::slide_order::{bounding, reading_order, Kind, Rect, Shape};

/// A slide of a presentation, as listed in [`Document::slides`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlideInfo {
    /// Its position in the slide order, from 1.
    pub number: usize,
    /// The text of its title (empty when it has none).
    pub title: String,
    /// `show="0"`: the slide is skipped in a slide show.
    pub hidden: bool,
}

/// The most bytes of a relationships part read (they are tiny; this bounds a forged one).
const REL_CAP: u64 = 4 * 1024 * 1024;
/// Most text characters kept for a slide title.
const TITLE_CHARS: usize = 200;
/// Most nodes of a SmartArt text shape / data point.
const SMART_NODES: usize = 20_000;
/// Most SmartArt text shapes read from one diagram.
const SMART_SHAPES: usize = 500;
/// Deepest group nesting followed.
const MAX_GROUP_DEPTH: usize = 20;
/// The namespaces (prefixes as PowerPoint writes them) whose content the slide reader reads inside
/// an `mc:Choice`: the DrawingML ones and the `a14` formulas (`a14:m`). Ink (`p14` content parts),
/// chart extensions (`cx*`) and the like are not read: their `Fallback` (a picture) is.
const PPTX_READS: &[&str] = &["a", "p", "r", "c", "dgm", "dsp", "m", "a14"];

/// Loads and converts a presentation.
#[cfg_attr(not(test), allow(dead_code))]
pub fn load_presentation(path: &Path, opts: &DocOptions) -> Result<Document, OfficeError> {
    load_presentation_cancellable(path, opts, None)
}

/// [`load_presentation`] that stops (with [`Document::truncated`] set) once `cancel` says so.
pub fn load_presentation_cancellable(
    path: &Path,
    opts: &DocOptions,
    cancel: Option<&Cancel>,
) -> Result<Document, OfficeError> {
    let names = container::inspect_word_package(path, &opts.limits)?;
    if names.iter().any(|n| n == "mimetype") {
        // An OpenDocument package: encrypted ones are told apart (`odt::odp` checks the type).
        if container::odf_encrypted(path, &names, &opts.limits) {
            return Err(OfficeError::Encrypted);
        }
        drop(names);
        return match crate::preview::markdown::catch_silent(|| {
            odt::odp::convert(path, opts, cancel)
        }) {
            Some(r) => r,
            None => Err(OfficeError::Corrupt(
                "panic while reading the presentation".into(),
            )),
        };
    }
    drop(names);
    match crate::preview::markdown::catch_silent(|| convert(path, opts, cancel)) {
        Some(r) => r,
        None => Err(OfficeError::Corrupt(
            "panic while reading the presentation".into(),
        )),
    }
}

// ---------------------------------------------------------------------------------------------
// bullets
// ---------------------------------------------------------------------------------------------

/// What a paragraph's bullet is.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Bu {
    /// `a:buNone`
    No,
    /// `a:buChar` / `a:buBlip`: drawn as a Markdown bullet whatever the glyph.
    Char,
    /// `a:buAutoNum`: the type (`arabicPeriod` ..) and the number it starts at.
    Auto(String, u32),
}

type Levels = [Option<Bu>; 9];

/// The bullet a paragraph-properties element (`a:pPr`, `a:lvl1pPr` ..) sets.
fn bullet_of(ppr: &Node) -> Option<Bu> {
    for c in ppr.nodes() {
        match c.name.as_str() {
            "buNone" => return Some(Bu::No),
            "buChar" | "buBlip" => return Some(Bu::Char),
            "buAutoNum" => {
                let ty = c.attr("type").unwrap_or("arabicPeriod").to_string();
                let start = c
                    .attr("startAt")
                    .and_then(|s| s.trim().parse::<u32>().ok())
                    .unwrap_or(1)
                    .clamp(1, 999_999);
                return Some(Bu::Auto(ty, start));
            }
            _ => {}
        }
    }
    None
}

/// The bullets of the nine levels of a list style (`a:lstStyle`, `p:titleStyle` ..).
fn levels_of(lst: &Node) -> Levels {
    let mut out: Levels = Default::default();
    for c in lst.nodes() {
        let Some(k) = c
            .name
            .strip_prefix("lvl")
            .and_then(|r| r.strip_suffix("pPr"))
            .and_then(|k| k.parse::<usize>().ok())
            .filter(|k| (1..=9).contains(k))
        else {
            continue;
        };
        out[k - 1] = bullet_of(c);
    }
    out
}

/// The label of an automatic number, and whether Markdown can number it itself (`N.`).
fn auto_label(ty: &str, n: u32) -> (String, bool) {
    let (fmt, plain_decimal_period) = if ty.starts_with("alphaLc") {
        ("lowerLetter", false)
    } else if ty.starts_with("alphaUc") {
        ("upperLetter", false)
    } else if ty.starts_with("romanLc") {
        ("lowerRoman", false)
    } else if ty.starts_with("romanUc") {
        ("upperRoman", false)
    } else if ty.starts_with("circleNum") {
        ("decimalEnclosedCircle", false)
    } else {
        ("decimal", ty == "arabicPeriod" || !ty.starts_with("arabic"))
    };
    let (open, close) = if ty.ends_with("ParenBoth") {
        ("(", ")")
    } else if ty.ends_with("ParenR") {
        ("", ")")
    } else if ty.ends_with("Plain") || ty.starts_with("circleNum") {
        ("", "")
    } else {
        ("", ".")
    };
    (
        format!("{open}{}{close}", format_number(fmt, n)),
        plain_decimal_period && fmt == "decimal" && close == ".",
    )
}

// ---------------------------------------------------------------------------------------------
// layouts and masters: what a placeholder inherits
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Class {
    Title,
    Sub,
    Body,
    /// Date, footer, slide number, header: not part of the slide's content.
    Furniture,
}

fn class_of(typ: Option<&str>) -> Class {
    match typ.unwrap_or("obj") {
        "title" | "ctrTitle" => Class::Title,
        "subTitle" => Class::Sub,
        "dt" | "ftr" | "sldNum" | "hdr" | "sldImg" => Class::Furniture,
        _ => Class::Body,
    }
}

/// A placeholder reference (`p:ph`).
#[derive(Debug, Clone)]
struct PhKey {
    class: Class,
    idx: String,
    /// The placeholder states an `idx` other than 0: it stands for the layout's placeholder of that
    /// `idx` and for no other (one the layout lacks inherits nothing). Without one, the kind of
    /// placeholder (`type`) finds its layout placeholder.
    explicit: bool,
}

fn ph_key(ph: &Node) -> PhKey {
    let idx = ph.attr("idx").unwrap_or("0").trim().to_string();
    PhKey {
        class: class_of(ph.attr("type")),
        explicit: idx != "0",
        idx,
    }
}

#[derive(Debug, Clone)]
struct PhInfo {
    class: Class,
    idx: String,
    rect: Option<Rect>,
    levels: Levels,
}

#[derive(Debug, Default)]
struct PartInfo {
    phs: Vec<PhInfo>,
    /// `titleStyle`, `bodyStyle`, `otherStyle` of a master.
    styles: [Levels; 3],
}

/// A layout with its master.
#[derive(Debug, Default)]
struct Inherit {
    layout: PartInfo,
    master: Rc<PartInfo>,
}

impl Inherit {
    /// The layout's placeholder a slide placeholder stands for: with an explicit `idx`, the one of
    /// that `idx` (of the same kind if there are several), and none when the layout has no such
    /// `idx`; without one, the same kind.
    fn layout_ph(&self, key: &PhKey) -> Option<&PhInfo> {
        let phs = &self.layout.phs;
        let same_idx = phs
            .iter()
            .find(|p| p.idx == key.idx && p.class == key.class)
            .or_else(|| phs.iter().find(|p| p.idx == key.idx && key.explicit));
        if key.explicit {
            same_idx
        } else {
            same_idx.or_else(|| phs.iter().find(|p| p.class == key.class))
        }
    }

    /// The master's placeholder of that kind (a subtitle is a body there).
    fn master_ph(&self, class: Class) -> Option<&PhInfo> {
        let want = if class == Class::Sub {
            Class::Body
        } else {
            class
        };
        self.master.phs.iter().find(|p| p.class == want)
    }
}

/// An integer attribute, 0 when absent or not a number.
fn int_attr(n: &Node, name: &str) -> i64 {
    n.attr(name)
        .and_then(|v| v.trim().parse::<i64>().ok())
        .unwrap_or(0)
}

/// The rectangle an `a:xfrm` / `p:xfrm` gives (a quarter turn swaps the sides: the bounding box).
fn xfrm_rect(x: &Node) -> Option<Rect> {
    let off = x.child("off")?;
    let ext = x.child("ext")?;
    let (mut w, mut h) = (int_attr(ext, "cx"), int_attr(ext, "cy"));
    let (mut ox, mut oy) = (int_attr(off, "x"), int_attr(off, "y"));
    let rot = int_attr(x, "rot").rem_euclid(21_600_000);
    if (2_700_000..8_100_000).contains(&rot) || (13_500_000..18_900_000).contains(&rot) {
        // (Attributes are 64-bit: a forged value must not overflow the arithmetic.)
        let (cx, cy) = (ox.saturating_add(w / 2), oy.saturating_add(h / 2));
        std::mem::swap(&mut w, &mut h);
        ox = cx.saturating_sub(w / 2);
        oy = cy.saturating_sub(h / 2);
    }
    Some(Rect::new(ox, oy, w, h))
}

/// The placeholder information a layout / master shape gives.
fn ph_info(n: &Node) -> Option<PhInfo> {
    let (nv, xfrm_parent) = match n.name.as_str() {
        "sp" => (n.child("nvSpPr")?, n.child("spPr")),
        "pic" => (n.child("nvPicPr")?, n.child("spPr")),
        "graphicFrame" => (n.child("nvGraphicFramePr")?, Some(n)),
        _ => return None,
    };
    let ph = nv.child("nvPr")?.child("ph")?;
    let key = ph_key(ph);
    let rect = xfrm_parent
        .and_then(|p| p.child("xfrm"))
        .and_then(xfrm_rect);
    let levels = n
        .child("txBody")
        .and_then(|t| t.child("lstStyle"))
        .map(levels_of)
        .unwrap_or_default();
    Some(PhInfo {
        class: key.class,
        idx: key.idx,
        rect,
        levels,
    })
}

fn tx_styles(n: &Node) -> [Levels; 3] {
    let get = |name: &str| n.child(name).map(levels_of).unwrap_or_default();
    [get("titleStyle"), get("bodyStyle"), get("otherStyle")]
}

// ---------------------------------------------------------------------------------------------
// walking a part
// ---------------------------------------------------------------------------------------------

#[derive(Default)]
struct WalkOut {
    /// The root's `show` attribute.
    show: Option<String>,
    /// The part held more than the node budget allows: the rest of it was not read.
    truncated: bool,
}

/// Reads the root `root`'s `p:cSld/p:spTree` one shape at a time, handing each to `on_shape`, and
/// the root's direct children named in `other` to `on_other`. Everything else is skipped without
/// being built. Stops (`truncated`) when `budget` runs out.
fn walk_part(
    bytes: &[u8],
    root: &str,
    budget: &mut Budget,
    on_shape: &mut dyn FnMut(Node),
    on_other: &mut dyn FnMut(Node),
    other: &[&str],
) -> Result<WalkOut, OfficeError> {
    let mut rd = XmlReader::new(bytes);
    let mut buf = Vec::new();
    // The elements descended into: the root, `cSld`, `spTree`.
    let mut path: Vec<String> = Vec::new();
    let mut out = WalkOut::default();
    loop {
        buf.clear();
        let (e, empty) = match rd.read_event_into(&mut buf).map_err(xml_err)? {
            Event::Start(e) => (e.into_owned(), false),
            Event::Empty(e) => (e.into_owned(), true),
            Event::End(_) => {
                path.pop();
                continue;
            }
            Event::Eof => break,
            _ => continue,
        };
        let name = String::from_utf8_lossy(e.local_name().as_ref()).into_owned();
        match path.len() {
            0 => {
                if name != root {
                    return Err(OfficeError::Unsupported);
                }
                out.show = attr(&e, b"show", false);
                if empty {
                    break;
                }
                path.push(name);
            }
            1 => {
                if name == "cSld" {
                    if !empty {
                        path.push(name);
                    }
                } else if other.contains(&name.as_str()) {
                    match read_element(&mut rd, &e, empty, budget)? {
                        Tree::Ok(n) => on_other(n),
                        Tree::TooBig => {
                            out.truncated = true;
                            return Ok(out);
                        }
                    }
                } else if !empty {
                    skip_rest(&mut rd)?;
                }
            }
            2 => {
                if name == "spTree" {
                    if !empty {
                        path.push(name);
                    }
                } else if !empty {
                    skip_rest(&mut rd)?;
                }
            }
            _ => {
                if matches!(name.as_str(), "nvGrpSpPr" | "grpSpPr") {
                    if !empty {
                        skip_rest(&mut rd)?;
                    }
                } else {
                    match read_element(&mut rd, &e, empty, budget)? {
                        Tree::Ok(n) => on_shape(n),
                        Tree::TooBig => {
                            out.truncated = true;
                            return Ok(out);
                        }
                    }
                }
            }
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------------------------
// the shapes of a slide
// ---------------------------------------------------------------------------------------------

/// Group transform: child space to absolute.
#[derive(Clone, Copy)]
struct Map {
    ox: i64,
    oy: i64,
    chx: i64,
    chy: i64,
    sx: f64,
    sy: f64,
}

impl Map {
    const IDENTITY: Map = Map {
        ox: 0,
        oy: 0,
        chx: 0,
        chy: 0,
        sx: 1.0,
        sy: 1.0,
    };

    fn apply(&self, r: Rect) -> Rect {
        let f = |v: i64, o: i64, ch: i64, s: f64| -> i64 {
            o.saturating_add(((v.saturating_sub(ch)) as f64 * s).round() as i64)
        };
        Rect::new(
            f(r.x, self.ox, self.chx, self.sx),
            f(r.y, self.oy, self.chy, self.sy),
            (r.w as f64 * self.sx).round() as i64,
            (r.h as f64 * self.sy).round() as i64,
        )
    }
}

/// A shape ready to be written.
struct Sh<'n> {
    rect: Option<Rect>,
    kind: Kind,
    body: ShBody<'n>,
}

enum ShBody<'n> {
    Text {
        tx: &'n Node,
        levels: Box<Levels>,
    },
    Pic {
        rid: Option<String>,
        alt: String,
    },
    Table(&'n Node),
    Chart(Option<String>),
    Smart(Option<String>),
    /// An embedded object (`p:oleObj`) with no picture: what it is, by name.
    Object(String),
    Group(Vec<Sh<'n>>),
}

struct Build<'a> {
    inh: &'a Inherit,
    shapes: usize,
    max_shapes: usize,
    truncated: bool,
}

/// The nodes of a shape tree level as shapes, `AlternateContent` unwrapped to its `Choice`.
fn build<'n>(
    nodes: impl Iterator<Item = &'n Node> + Clone,
    map: &Map,
    b: &mut Build<'_>,
    depth: usize,
    out: &mut Vec<Sh<'n>>,
) {
    if depth > MAX_GROUP_DEPTH {
        // Shapes below the deepest level followed are left out: say so.
        // (`AlternateContent` is one of them: its `Choice` holds the shapes, so nesting it past the
        // limit drops the shapes inside as surely as nesting a group does.)
        if nodes.clone().any(|n| {
            matches!(
                n.name.as_str(),
                "sp" | "pic" | "graphicFrame" | "grpSp" | "AlternateContent"
            )
        }) {
            b.truncated = true;
        }
        return;
    }
    for n in nodes {
        if b.shapes >= b.max_shapes {
            b.truncated = true;
            return;
        }
        match n.name.as_str() {
            "sp" => {
                b.shapes += 1;
                if let Some(s) = build_sp(n, map, b.inh) {
                    out.push(s);
                }
            }
            "pic" => {
                b.shapes += 1;
                if let Some(s) = build_pic(n, map, b.inh) {
                    out.push(s);
                }
            }
            "graphicFrame" => {
                b.shapes += 1;
                if let Some(s) = build_frame(n, map, b.inh) {
                    out.push(s);
                }
            }
            "grpSp" => {
                b.shapes += 1;
                let gm = group_map(n, map);
                let mut kids = Vec::new();
                build(n.nodes(), &gm, b, depth + 1, &mut kids);
                if !kids.is_empty() {
                    let rect = bounding(kids.iter().filter_map(|k| k.rect));
                    out.push(Sh {
                        rect,
                        kind: Kind::Other,
                        body: ShBody::Group(kids),
                    });
                }
            }
            "AlternateContent" => {
                if let Some(c) = alt_content(n, PPTX_READS) {
                    build(c.nodes(), map, b, depth + 1, out);
                }
            }
            _ => {}
        }
    }
}

/// The transform a group applies to its members.
fn group_map(g: &Node, outer: &Map) -> Map {
    let Some(x) = g.child("grpSpPr").and_then(|p| p.child("xfrm")) else {
        return *outer;
    };
    let (Some(off), Some(ext)) = (x.child("off"), x.child("ext")) else {
        return *outer;
    };
    let abs = outer.apply(Rect::new(
        int_attr(off, "x"),
        int_attr(off, "y"),
        int_attr(ext, "cx"),
        int_attr(ext, "cy"),
    ));
    let (chx, chy, cw, ch) = match (x.child("chOff"), x.child("chExt")) {
        (Some(o), Some(e)) => (
            int_attr(o, "x"),
            int_attr(o, "y"),
            int_attr(e, "cx"),
            int_attr(e, "cy"),
        ),
        // No child space given: the members use the group's own space.
        _ => (abs.x, abs.y, abs.w, abs.h),
    };
    let scale = |a: i64, c: i64| {
        let s = if c > 0 { a as f64 / c as f64 } else { 1.0 };
        if s.is_finite() {
            s.clamp(0.0, 1.0e6)
        } else {
            1.0
        }
    };
    Map {
        ox: abs.x,
        oy: abs.y,
        chx,
        chy,
        sx: scale(abs.w, cw),
        sy: scale(abs.h, ch),
    }
}

/// The position of a shape: its own `xfrm` through the group transform, else the position its
/// placeholder inherits (layout, then master).
fn position(own: Option<Rect>, map: &Map, ph: Option<&PhKey>, inh: &Inherit) -> Option<Rect> {
    if let Some(r) = own {
        return Some(map.apply(r));
    }
    let key = ph?;
    match inh.layout_ph(key) {
        // The layout's placeholder, or the master's when the layout leaves its position to it.
        Some(p) => p
            .rect
            .or_else(|| inh.master_ph(p.class).and_then(|m| m.rect)),
        // An `idx` the layout does not have inherits no position.
        None if key.explicit => None,
        None => inh.master_ph(key.class).and_then(|p| p.rect),
    }
}

fn ph_of(nv: Option<&Node>) -> Option<&Node> {
    nv?.child("nvPr")?.child("ph")
}

fn has_text(tx: &Node) -> bool {
    let mut s = String::new();
    collect_t_text(tx, &mut s, 0);
    !s.trim().is_empty()
}

fn build_sp<'n>(sp: &'n Node, map: &Map, inh: &Inherit) -> Option<Sh<'n>> {
    let tx = sp.child("txBody")?;
    if !has_text(tx) {
        return None;
    }
    let ph = ph_of(sp.child("nvSpPr")).map(ph_key);
    let class = ph.as_ref().map(|k| k.class);
    if class == Some(Class::Furniture) {
        return None;
    }
    let own = sp
        .child("spPr")
        .and_then(|p| p.child("xfrm"))
        .and_then(xfrm_rect);
    let rect = position(own, map, ph.as_ref(), inh);
    let kind = match class {
        Some(Class::Title) => Kind::Title,
        Some(Class::Sub) => Kind::Subtitle,
        _ => Kind::Other,
    };
    // Bullets, level by level: the paragraph's own (applied later), the shape's list style, the
    // layout's placeholder, the master's placeholder, the master's text styles.
    let mine = tx.child("lstStyle").map(levels_of).unwrap_or_default();
    let lay = ph.as_ref().and_then(|k| inh.layout_ph(k));
    let mas = class.and_then(|c| inh.master_ph(c));
    let style = match class {
        Some(Class::Title) => Some(0),
        Some(Class::Body) => Some(1),
        Some(_) => None,
        None => Some(2),
    };
    let mut levels: Box<Levels> = Box::default();
    for k in 0..9 {
        levels[k] = mine[k]
            .clone()
            .or_else(|| lay.and_then(|p| p.levels[k].clone()))
            .or_else(|| mas.and_then(|p| p.levels[k].clone()))
            .or_else(|| style.and_then(|s| inh.master.styles[s][k].clone()));
    }
    Some(Sh {
        rect,
        kind,
        body: ShBody::Text { tx, levels },
    })
}

fn alt_of(cnv: Option<&Node>) -> String {
    let Some(c) = cnv else {
        return String::new();
    };
    let d = c.attr("descr").filter(|d| !d.trim().is_empty());
    let t = c.attr("title").filter(|d| !d.trim().is_empty());
    clean(d.or(t).unwrap_or(""))
        .chars()
        .take(300)
        .collect::<String>()
}

fn build_pic<'n>(pic: &'n Node, map: &Map, inh: &Inherit) -> Option<Sh<'n>> {
    let nv = pic.child("nvPicPr");
    let ph = ph_of(nv).map(ph_key);
    if ph.as_ref().map(|k| k.class) == Some(Class::Furniture) {
        return None;
    }
    let blip = pic.child("blipFill").and_then(|b| b.child("blip"))?;
    let rid = blip.rel_attr("embed").map(str::to_string);
    if rid.is_none() && blip.rel_attr("link").is_none() {
        return None;
    }
    let own = pic
        .child("spPr")
        .and_then(|p| p.child("xfrm"))
        .and_then(xfrm_rect);
    Some(Sh {
        rect: position(own, map, ph.as_ref(), inh),
        kind: Kind::Other,
        body: ShBody::Pic {
            rid,
            alt: alt_of(nv.and_then(|n| n.child("cNvPr"))),
        },
    })
}

/// The first picture reference under `n` (an embedded object's fallback picture).
fn find_blip(n: &Node, depth: usize) -> Option<String> {
    if depth > 12 {
        return None;
    }
    for c in n.nodes() {
        if c.name == "blip" {
            if let Some(id) = c.rel_attr("embed") {
                return Some(id.to_string());
            }
        }
        if let Some(r) = find_blip(c, depth + 1) {
            return Some(r);
        }
    }
    None
}

/// The name of the first embedded object (`p:oleObj`) under `n`: its `progId` (`Word.Document.12`),
/// else its `name`, else empty.
fn find_ole(n: &Node, depth: usize) -> Option<String> {
    if depth > 12 {
        return None;
    }
    for c in n.nodes() {
        if c.name == "oleObj" {
            let id = c
                .attr("progId")
                .or_else(|| c.attr("name"))
                .map(clean)
                .unwrap_or_default();
            return Some(id.trim().chars().take(80).collect());
        }
        if let Some(r) = find_ole(c, depth + 1) {
            return Some(r);
        }
    }
    None
}

fn build_frame<'n>(f: &'n Node, map: &Map, inh: &Inherit) -> Option<Sh<'n>> {
    let nv = f.child("nvGraphicFramePr");
    let ph = ph_of(nv).map(ph_key);
    let own = f.child("xfrm").and_then(xfrm_rect);
    let rect = position(own, map, ph.as_ref(), inh);
    let data = f.child("graphic")?.child("graphicData")?;
    let uri = data.attr("uri").unwrap_or("").to_ascii_lowercase();
    let alt = alt_of(nv.and_then(|n| n.child("cNvPr")));
    let body = if let Some(t) = data.child("tbl") {
        ShBody::Table(t)
    } else if uri.contains("/chart") || data.child("chart").is_some() {
        ShBody::Chart(
            data.child("chart")
                .and_then(|c| c.rel_attr("id"))
                .map(str::to_string),
        )
    } else if uri.ends_with("/diagram") {
        ShBody::Smart(
            data.child("relIds")
                .and_then(|c| c.rel_attr("dm"))
                .map(str::to_string),
        )
    } else if let Some(rid) = find_blip(data, 0) {
        ShBody::Pic {
            rid: Some(rid),
            alt,
        }
    } else if !alt.trim().is_empty() {
        ShBody::Pic { rid: None, alt }
    } else {
        // (A frame that is none of these and has no object is nothing to show.)
        ShBody::Object(find_ole(data, 0)?)
    };
    Some(Sh {
        rect,
        kind: Kind::Other,
        body,
    })
}

// ---------------------------------------------------------------------------------------------
// reading the package
// ---------------------------------------------------------------------------------------------

/// A slide read from the package.
struct Loaded {
    nodes: Vec<Node>,
    hidden: bool,
    rels: HashMap<String, Rel>,
    inh: Rc<Inherit>,
    notes: Option<String>,
    /// Shapes were left unread (the node budget or the shape cap).
    cut: bool,
}

struct Rd<'c, 'a> {
    c: &'c mut Conv<'a>,
    read_total: u64,
    layouts: HashMap<String, Rc<Inherit>>,
    masters: HashMap<String, Rc<PartInfo>>,
    next_list: u32,
    /// The slide's rectangle (`p:sldSz`): shapes wholly outside it are read last.
    slide_rect: Option<Rect>,
    /// `p:presentation firstSlideNum`: what the first slide's number field shows.
    first_num: i64,
    /// The number of the slide being written (from 1, its place in the order).
    cur_slide: usize,
    /// The slide just written when the next one is the same part (kept to be written again
    /// without reading its part and relationships again).
    last: Option<(String, Loaded)>,
    /// The shapes handed to the reading-order pass so far, over the whole deck.
    order_shapes: usize,
}

fn convert(
    path: &Path,
    opts: &DocOptions,
    cancel: Option<&Cancel>,
) -> Result<Document, OfficeError> {
    // The presentation part and its relationships are read under the size of a slide part, not
    // the package's (256 MiB): a forged 250 MB comment must not be read in.
    let cap = opts.max_slide_part_bytes.min(opts.limits.max_part_bytes);
    let mut pkg = Pkg::open(path)?;
    let root_rels = read_rels_of_root(&mut pkg, REL_CAP).unwrap_or_default();
    let main = root_rels
        .values()
        .find(|r| r.kind == "officeDocument" && !r.external)
        .map(|r| r.target.clone())
        .filter(|t| pkg.has(t))
        .or_else(|| {
            pkg.has("ppt/presentation.xml")
                .then(|| "ppt/presentation.xml".to_string())
        })
        .ok_or(OfficeError::Unsupported)?;
    let rels = read_rels_of(&mut pkg, &main, REL_CAP).unwrap_or_default();

    // The slide order: `p:sldIdLst` of the presentation, resolved through its relationships.
    let mut ids: Vec<String> = Vec::new();
    let mut over = false;
    let mut slide_rect: Option<Rect> = None;
    let mut first_num: i64 = 1;
    {
        let Some(r) = pkg.part(&main, cap)? else {
            return Err(OfficeError::Unsupported);
        };
        let mut rd = XmlReader::new(r);
        let mut buf = Vec::new();
        let mut root_seen = false;
        loop {
            buf.clear();
            match rd.read_event_into(&mut buf).map_err(xml_err)? {
                Event::Start(e) | Event::Empty(e) => {
                    let local = e.local_name();
                    if !root_seen {
                        root_seen = true;
                        // An xlsx / docx renamed .pptx is not a presentation.
                        if local.as_ref() != b"presentation" {
                            return Err(OfficeError::Unsupported);
                        }
                        first_num = attr(&e, b"firstSlideNum", false)
                            .and_then(|v| v.trim().parse::<i64>().ok())
                            .unwrap_or(1)
                            .clamp(0, 1_000_000);
                    } else if local.as_ref() == b"sldSz" {
                        let n = |k: &[u8]| attr(&e, k, false).and_then(|v| v.trim().parse().ok());
                        // (A size of zero or less is no size: it would put every shape off
                        // the slide.)
                        if let (Some(w), Some(h)) = (n(b"cx"), n(b"cy")) {
                            if w > 0 && h > 0 {
                                slide_rect = Some(Rect::new(0, 0, w, h));
                            }
                        }
                    } else if local.as_ref() == b"sldId" {
                        if let Some(id) = attr(&e, b"id", true) {
                            if ids.len() >= opts.max_slides {
                                over = true;
                                break;
                            }
                            ids.push(id);
                        }
                    }
                }
                Event::Eof => break,
                _ => {}
            }
        }
    }

    let media = Pkg::open(path)?;
    let mut conv = Conv::new(
        opts,
        cancel,
        Styles::default(),
        Numbering::default(),
        HashMap::new(),
        media,
    );
    conv.truncated |= over;
    conv.split_bullet_lists = true;
    let mut slides: Vec<SlideInfo> = Vec::new();
    {
        let mut rd = Rd {
            c: &mut conv,
            read_total: 0,
            layouts: HashMap::new(),
            masters: HashMap::new(),
            next_list: 0,
            slide_rect,
            first_num,
            cur_slide: 0,
            last: None,
            order_shapes: 0,
        };
        let parts: Vec<Option<String>> = ids
            .iter()
            .map(|rid| {
                rels.get(rid)
                    .filter(|r| r.kind == "slide" && !r.external)
                    .map(|r| r.target.clone())
            })
            .collect();
        for (i, part) in parts.iter().enumerate() {
            if rd.c.cancelled() || rd.c.full || rd.order_shapes > opts.max_deck_order_shapes {
                rd.c.truncated = true;
                break;
            }
            // A slide listed again right after itself is parsed once.
            let keep = part.is_some() && parts.get(i + 1) == Some(part);
            if let Some(info) = rd.slide(i + 1, part.clone(), keep) {
                slides.push(info);
            }
        }
    }
    let mut doc = conv.assemble(Vec::new());
    doc.slides = slides;
    Ok(doc)
}

impl Rd<'_, '_> {
    /// An XML part of the package, within the per-part and the total read budgets (`None` when it
    /// is missing, unreadable, or over a budget -- the last sets `truncated`).
    fn read_part(&mut self, part: &str) -> Option<Vec<u8>> {
        self.read_part_capped(part, u64::MAX)
    }

    /// [`Self::read_part`] with a smaller per-part cap.
    fn read_part_capped(&mut self, part: &str, cap: u64) -> Option<Vec<u8>> {
        let opts = self.c.opts;
        let left = opts.max_pptx_read_total.saturating_sub(self.read_total);
        let limit = opts.max_slide_part_bytes.min(left).min(cap);
        if limit == 0 || self.c.cancelled() {
            self.c.truncated = true;
            return None;
        }
        let mut bytes = Vec::new();
        {
            let r = self.c.media.part(part, limit + 1).ok()??;
            r.take(limit + 1).read_to_end(&mut bytes).ok()?;
        }
        if bytes.len() as u64 > limit {
            self.c.truncated = true;
            return None;
        }
        self.read_total += bytes.len() as u64;
        Some(bytes)
    }

    /// The relationships of a part, read under the same budgets as any other part of the deck (a
    /// forged one of megabytes costs as much as a slide of that size).
    fn rels_of(&mut self, part: &str) -> HashMap<String, Rel> {
        let (dir, rp) = rels_path(part);
        match self.read_part_capped(&rp, REL_CAP) {
            Some(b) => parse_rels(&b[..], &dir).unwrap_or_default(),
            None => HashMap::new(),
        }
    }

    /// A layout with its master, read once.
    fn inherit(&mut self, layout_part: &str) -> Rc<Inherit> {
        if let Some(i) = self.layouts.get(layout_part) {
            return Rc::clone(i);
        }
        let layout = self
            .read_part(layout_part)
            .and_then(|b| part_info(&b, "sldLayout", self.c.opts).ok())
            .unwrap_or_default();
        let lrels = self.rels_of(layout_part);
        let master_part = lrels
            .values()
            .find(|r| r.kind == "slideMaster" && !r.external)
            .map(|r| r.target.clone());
        let master = match master_part {
            Some(mp) => match self.masters.get(&mp) {
                Some(m) => Rc::clone(m),
                None => {
                    let info = self
                        .read_part(&mp)
                        .and_then(|b| part_info(&b, "sldMaster", self.c.opts).ok())
                        .unwrap_or_default();
                    let m = Rc::new(info);
                    self.masters.insert(mp, Rc::clone(&m));
                    m
                }
            },
            None => Rc::new(PartInfo::default()),
        };
        let inh = Rc::new(Inherit { layout, master });
        self.layouts
            .insert(layout_part.to_string(), Rc::clone(&inh));
        inh
    }

    fn load_slide(&mut self, part: &str) -> Option<Loaded> {
        let bytes = self.read_part(part)?;
        let rels = self.rels_of(part);
        let opts = self.c.opts;
        let mut budget = Budget::new(opts.max_block_nodes, opts.max_block_text_bytes);
        let mut nodes: Vec<Node> = Vec::new();
        let mut cut = false;
        let max = opts.max_slide_shapes;
        let walked = walk_part(
            &bytes,
            "sld",
            &mut budget,
            &mut |n| {
                if nodes.len() < max {
                    nodes.push(n);
                } else {
                    cut = true;
                }
            },
            &mut |_| {},
            &[],
        );
        drop(bytes);
        let walked = walked.ok()?;
        let layout_part = rels
            .values()
            .find(|r| r.kind == "slideLayout" && !r.external)
            .map(|r| r.target.clone());
        let inh = match layout_part {
            Some(lp) => self.inherit(&lp),
            None => Rc::new(Inherit::default()),
        };
        let notes = rels
            .values()
            .find(|r| r.kind == "notesSlide" && !r.external)
            .map(|r| r.target.clone());
        Some(Loaded {
            nodes,
            hidden: walked
                .show
                .as_deref()
                .is_some_and(|s| matches!(s.trim(), "0" | "false")),
            rels,
            inh,
            notes,
            cut: cut || walked.truncated,
        })
    }

    /// Writes one slide; `None` when not even its heading fit the output budgets.
    ///
    /// `keep`: the next slide is this same part, so the parsed slide is kept for it.
    fn slide(&mut self, number: usize, part: Option<String>, keep: bool) -> Option<SlideInfo> {
        self.cur_slide = number;
        let cached = match (&part, self.last.take()) {
            (Some(p), Some((lp, l))) if *p == lp => Some(l),
            _ => None,
        };
        let loaded = cached.or_else(|| part.as_deref().and_then(|p| self.load_slide(p)));
        let Some(mut l) = loaded else {
            // A slide that cannot be read still has its place in the order.
            self.c.truncated = true;
            return self.write_heading(number, "", false);
        };
        if l.cut {
            self.c.truncated = true;
        }
        self.c.rels = std::mem::take(&mut l.rels);
        let info = self.write_slide(number, &l);
        l.rels = std::mem::take(&mut self.c.rels);
        if let (true, Some(p)) = (keep, part) {
            self.last = Some((p, l));
        }
        info
    }

    fn write_slide(&mut self, number: usize, l: &Loaded) -> Option<SlideInfo> {
        let mut b = Build {
            inh: &l.inh,
            shapes: 0,
            max_shapes: self.c.opts.max_slide_shapes,
            truncated: false,
        };
        let mut items: Vec<Sh<'_>> = Vec::new();
        build(l.nodes.iter(), &Map::IDENTITY, &mut b, 0, &mut items);
        if b.truncated {
            self.c.truncated = true;
        }
        // The first title shape is the heading; any other is text.
        let title_idx = items
            .iter()
            .position(|s| s.kind == Kind::Title && matches!(s.body, ShBody::Text { .. }));
        let title = title_idx.map_or_else(String::new, |i| match &items[i].body {
            ShBody::Text { tx, .. } => plain_title(tx),
            _ => String::new(),
        });
        let info = self.write_heading(number, &title, l.hidden)?;
        if let Some(i) = title_idx {
            // (A title kept as text must not claim the first place twice.)
            for (k, s) in items.iter_mut().enumerate() {
                if s.kind == Kind::Title && k != i {
                    s.kind = Kind::Subtitle;
                }
            }
        }
        self.write_items(&items, title_idx, 0);
        if let Some(np) = l.notes.as_deref() {
            self.write_notes(np);
        }
        Some(info)
    }

    fn write_heading(&mut self, number: usize, title: &str, hidden: bool) -> Option<SlideInfo> {
        write_heading(self.c, number, title, hidden)
    }

    // -----------------------------------------------------------------------------------------
    // body
    // -----------------------------------------------------------------------------------------

    fn write_items(&mut self, items: &[Sh<'_>], skip: Option<usize>, depth: usize) {
        if depth > MAX_GROUP_DEPTH {
            return;
        }
        // The reading-order pass is paid for per shape over the whole deck: past the budget the
        // rest of the deck is left out (the slide that crossed it keeps its heading).
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
            if self.c.cancelled() || self.c.full {
                self.c.truncated = true;
                return;
            }
            match &items[i].body {
                ShBody::Text { tx, levels } => {
                    let blks = self.text_blocks(tx, levels);
                    self.c.write_blocks(blks);
                }
                ShBody::Pic { rid, alt } => {
                    self.c.cur_ctx = Ctx::Body;
                    let md = match rid {
                        Some(r) => self.c.image_md(r, alt),
                        None => self.c.placeholder(alt),
                    };
                    self.c.write_block(Blk::Para(md));
                }
                ShBody::Table(t) => {
                    if let Some(b) = self.table(t) {
                        self.c.write_block(b);
                    }
                }
                ShBody::Chart(rid) => {
                    self.c.cur_ctx = Ctx::Body;
                    let title = rid.as_deref().and_then(|r| self.c.chart_title(r));
                    let label = match title {
                        Some(t) => format!("chart: {t}"),
                        None => "chart".to_string(),
                    };
                    let md = self.c.placeholder(&label);
                    self.c.write_block(Blk::Para(md));
                }
                ShBody::Smart(rid) => {
                    let blks = self.smartart(rid.as_deref());
                    self.c.write_blocks(blks);
                }
                ShBody::Object(name) => {
                    self.c.cur_ctx = Ctx::Body;
                    let label = if name.is_empty() {
                        "object".to_string()
                    } else {
                        format!("object: {name}")
                    };
                    let md = self.c.placeholder(&label);
                    self.c.write_block(Blk::Para(md));
                }
                ShBody::Group(kids) => self.write_items(kids, None, depth + 1),
            }
        }
    }

    /// The paragraphs of a text body as blocks: bullets and numbers from `levels` (the shape's
    /// inherited bullets) unless the paragraph sets its own.
    fn text_blocks(&mut self, tx: &Node, levels: &Levels) -> Vec<Blk> {
        self.c.cur_ctx = Ctx::Body;
        self.next_list += 1;
        let list = self.next_list;
        let mut counters: [Option<(String, u32)>; 9] = Default::default();
        let mut out = Vec::new();
        for p in tx.nodes().filter(|n| n.name == "p") {
            let ppr = p.child("pPr");
            let lvl = ppr.map(|n| int_attr(n, "lvl")).unwrap_or(0).clamp(0, 8) as usize;
            let bu = ppr.and_then(bullet_of).or_else(|| levels[lvl].clone());
            let inl = self.paragraph(p);
            if !visible(&inl) {
                continue;
            }
            for c in counters.iter_mut().skip(lvl + 1) {
                *c = None;
            }
            let list_role = match bu {
                Some(Bu::Char) => {
                    counters[lvl] = None;
                    Some(ListRole {
                        level: lvl as u8,
                        list,
                        marker: Marker::Bullet,
                        label: String::new(),
                    })
                }
                Some(Bu::Auto(ty, start)) => {
                    // The count goes on only while the scheme stays the same.
                    let n = match &counters[lvl] {
                        Some((t, c)) if *t == ty => c.saturating_add(1),
                        _ => start,
                    };
                    counters[lvl] = Some((ty.clone(), n));
                    let (label, markdown_can) = auto_label(&ty, n);
                    Some(ListRole {
                        level: lvl as u8,
                        list,
                        marker: if markdown_can {
                            Marker::Ordered(n)
                        } else {
                            Marker::Literal
                        },
                        label,
                    })
                }
                _ => {
                    counters[lvl] = None;
                    None
                }
            };
            let role = Role {
                heading: None,
                code: false,
                list: list_role,
            };
            out.extend(self.c.make_blocks(inl.segs, &role, Vec::new(), Ctx::Body));
        }
        out
    }

    /// The inline content of an `a:p`.
    fn paragraph(&mut self, p: &Node) -> Inl {
        let mut inl = Inl::default();
        let mut link: Option<String> = None;
        for n in p.nodes() {
            self.inline(n, &mut inl, &mut link, 0);
        }
        if link.is_some() {
            inl.segs.push(Seg::LinkClose);
        }
        hoist_link_space(&mut inl.segs);
        inl
    }

    fn inline(&mut self, n: &Node, inl: &mut Inl, link: &mut Option<String>, depth: usize) {
        if depth > 8 {
            // Content nested past the limit is dropped: say so (an element that is read at no
            // depth, such as run properties, does not count).
            if matches!(
                n.name.as_str(),
                "r" | "fld" | "br" | "AlternateContent" | "m" | "oMath" | "oMathPara"
            ) {
                self.c.truncated = true;
            }
            return;
        }
        match n.name.as_str() {
            "r" | "fld" => self.run(n, inl, link),
            "br" => {
                if link.take().is_some() {
                    inl.segs.push(Seg::LinkClose);
                }
                inl.segs.push(Seg::Break);
            }
            "AlternateContent" => {
                if let Some(c) = alt_content(n, PPTX_READS) {
                    for k in c.nodes() {
                        self.inline(k, inl, link, depth + 1);
                    }
                }
            }
            // `a14:m` holds the formula.
            "m" => {
                for k in n.nodes() {
                    self.inline(k, inl, link, depth + 1);
                }
            }
            "oMath" => self.c.math(n, false, inl),
            "oMathPara" => self.c.math(n, true, inl),
            _ => {}
        }
    }

    fn run(&mut self, r: &Node, inl: &mut Inl, link: &mut Option<String>) {
        let rpr = r.child("rPr");
        let on = |name: &str| {
            rpr.and_then(|p| p.attr(name))
                .is_some_and(|v| matches!(v.trim(), "1" | "true"))
        };
        let baseline = rpr.map_or(0, |p| int_attr(p, "baseline"));
        let f = F {
            b: on("b"),
            i: on("i"),
            s: rpr
                .and_then(|p| p.attr("strike"))
                .is_some_and(|v| matches!(v.trim(), "sngStrike" | "dblStrike")),
            v: match baseline {
                b if b > 0 => Vert::Sup,
                b if b < 0 => Vert::Sub,
                _ => Vert::Base,
            },
        };
        let url = rpr
            .and_then(|p| p.child("hlinkClick"))
            .and_then(|h| h.rel_attr("id"))
            .and_then(|id| self.c.rels.get(id))
            .filter(|rel| rel.external)
            .and_then(|rel| safe_url(&rel.target));
        if url != *link {
            if link.is_some() {
                inl.segs.push(Seg::LinkClose);
            }
            if let Some(u) = &url {
                inl.segs.push(Seg::LinkOpen(u.clone()));
            }
            *link = url;
        }
        // A slide-number field shows the number of this slide, not the one stored (`‹#›`, or the
        // number it had when the file was saved).
        if r.name == "fld" && r.attr("type").is_some_and(|t| t.trim() == "slidenum") {
            let n = self.first_num.saturating_add(self.cur_slide as i64 - 1);
            inl.segs.push(Seg::Text(n.to_string(), f));
            return;
        }
        for t in r.nodes().filter(|n| n.name == "t") {
            // A vertical tab is a line break (PowerPoint writes one for a soft return in old files).
            let text = t.text();
            let mut first = true;
            for part in text.split('\u{B}') {
                if !first {
                    inl.segs.push(Seg::Break);
                }
                first = false;
                let part = clean(part);
                if !part.is_empty() {
                    inl.segs.push(Seg::Text(part, f));
                }
            }
        }
    }

    fn table(&mut self, t: &Node) -> Option<Blk> {
        let mut rows: Vec<Vec<String>> = Vec::new();
        let mut cells = 0usize;
        'rows: for tr in t.nodes().filter(|n| n.name == "tr") {
            let mut row: Vec<String> = Vec::new();
            for tc in tr.nodes().filter(|n| n.name == "tc") {
                let flag = |name: &str| matches!(tc.attr(name).map(str::trim), Some("1" | "true"));
                // (PowerPoint writes every grid column: a cell spanning two columns is followed by
                // an `hMerge` cell, so `gridSpan` itself adds nothing.)
                cells += 1;
                if cells > self.c.opts.max_table_cells {
                    self.c.truncated = true;
                    break 'rows;
                }
                let text = if flag("hMerge") || flag("vMerge") {
                    String::new()
                } else {
                    self.c.cur_ctx = Ctx::Cell;
                    let mut blks = Vec::new();
                    if let Some(tx) = tc.child("txBody") {
                        for p in tx.nodes().filter(|n| n.name == "p") {
                            let inl = self.paragraph(p);
                            let role = Role {
                                heading: None,
                                code: false,
                                list: None,
                            };
                            blks.extend(self.c.make_blocks(inl.segs, &role, Vec::new(), Ctx::Cell));
                        }
                    }
                    self.c.cur_ctx = Ctx::Body;
                    cell_text(&blks, "<br>")
                };
                row.push(text);
            }
            if !row.is_empty() {
                rows.push(row);
            }
        }
        (!rows.is_empty()).then_some(Blk::Table(rows))
    }

    // -----------------------------------------------------------------------------------------
    // SmartArt
    // -----------------------------------------------------------------------------------------

    /// The text of a SmartArt graphic as a bullet list (a placeholder when the file holds none).
    fn smartart(&mut self, dm_rid: Option<&str>) -> Vec<Blk> {
        let texts = dm_rid.map(|r| self.smart_texts(r)).unwrap_or_default();
        if texts.is_empty() {
            self.c.cur_ctx = Ctx::Body;
            let md = self.c.placeholder("SmartArt");
            return vec![Blk::Para(md)];
        }
        self.next_list += 1;
        let list = self.next_list;
        texts
            .into_iter()
            .map(|paras| {
                let mut segs = Vec::new();
                for (i, p) in paras.iter().enumerate() {
                    if i > 0 {
                        segs.push(Seg::Break);
                    }
                    segs.push(Seg::Text(p.clone(), F::default()));
                }
                Blk::Item(Item {
                    level: 0,
                    list,
                    marker: Marker::Bullet,
                    label: String::new(),
                    text: guard_rule(emit(&segs, true, false).trim().to_string()),
                })
            })
            .collect()
    }

    /// The paragraphs of each text shape: from the diagram's drawing (what PowerPoint drew, in
    /// reading order), else from its data model.
    fn smart_texts(&mut self, dm_rid: &str) -> Vec<Vec<String>> {
        let Some(dm) = self
            .c
            .rels
            .get(dm_rid)
            .filter(|r| !r.external)
            .map(|r| r.target.clone())
        else {
            return Vec::new();
        };
        let Some(bytes) = self.read_part(&dm) else {
            return Vec::new();
        };
        let (data_texts, drawing_rid) = scan_diagram_data(&bytes);
        drop(bytes);
        if let Some(rid) = drawing_rid {
            let part = self
                .c
                .rels
                .get(&rid)
                .filter(|r| !r.external)
                .map(|r| r.target.clone());
            if let Some(bytes) = part.and_then(|p| self.read_part(&p)) {
                let shapes = scan_diagram_drawing(&bytes);
                if !shapes.is_empty() {
                    let order: Vec<Shape> = shapes
                        .iter()
                        .map(|(r, _)| Shape {
                            rect: *r,
                            kind: Kind::Other,
                        })
                        .collect();
                    return reading_order(&order, None)
                        .into_iter()
                        .map(|i| shapes[i].1.clone())
                        .collect();
                }
            }
        }
        data_texts
    }

    // -----------------------------------------------------------------------------------------
    // notes
    // -----------------------------------------------------------------------------------------

    /// The speaker notes of a slide: the notes page's body placeholder, as a quote block.
    fn write_notes(&mut self, part: &str) {
        let Some(bytes) = self.read_part(part) else {
            return;
        };
        let rels = self.rels_of(part);
        let opts = self.c.opts;
        let mut budget = Budget::new(opts.max_block_nodes, opts.max_block_text_bytes);
        let mut bodies: Vec<Node> = Vec::new();
        let mut over = false;
        let walked = walk_part(
            &bytes,
            "notes",
            &mut budget,
            &mut |n| {
                let is_body = n.name == "sp"
                    && ph_of(n.child("nvSpPr")).is_some_and(|p| p.attr("type") == Some("body"));
                if is_body {
                    if bodies.len() < 8 {
                        bodies.push(n);
                    } else {
                        over = true;
                    }
                }
            },
            &mut |_| {},
            &[],
        );
        drop(bytes);
        if walked.as_ref().map_or(true, |w| w.truncated) || over {
            self.c.truncated = true;
        }
        let slide_rels = std::mem::replace(&mut self.c.rels, rels);
        let mut lines: Vec<String> = Vec::new();
        for sp in &bodies {
            let Some(tx) = sp.child("txBody") else {
                continue;
            };
            self.c.cur_ctx = Ctx::Note;
            for p in tx.nodes().filter(|n| n.name == "p") {
                let inl = self.paragraph(p);
                if !visible(&inl) {
                    continue;
                }
                let role = Role {
                    heading: None,
                    code: false,
                    list: None,
                };
                for b in self.c.make_blocks(inl.segs, &role, Vec::new(), Ctx::Note) {
                    if let Blk::Para(t) = b {
                        lines.push(t);
                    }
                }
            }
        }
        self.c.cur_ctx = Ctx::Body;
        self.c.rels = slide_rels;
        write_notes_quote(self.c, lines);
    }
}

/// Writes the heading of a slide (`## Slide 3: Title`, with the hidden mark): `None` when not
/// even that fit the output budgets. Shared with the OpenDocument reader.
pub(super) fn write_heading(
    c: &mut Conv<'_>,
    number: usize,
    title: &str,
    hidden: bool,
) -> Option<SlideInfo> {
    let lang = c.opts.lang;
    let mut text = format!("{} {number}", tr(lang, Msg::SlideHeading));
    if !title.is_empty() {
        text.push_str(": ");
        text.push_str(&escape_heading(title));
    }
    if hidden {
        text.push_str(tr(lang, Msg::SlideHiddenSuffix));
    }
    let before = c.out.len();
    c.write_block(Blk::Heading {
        level: 2,
        text: text.clone(),
        plain: format!("{} {number}: {title}", tr(lang, Msg::SlideHeading)),
        bookmarks: Vec::new(),
    });
    c.flush_code();
    if c.out.len() == before {
        return None;
    }
    Some(SlideInfo {
        number,
        title: title.to_string(),
        hidden,
    })
}

/// The speaker notes of a slide as a quote block under it (nothing for no lines). Shared with the
/// OpenDocument reader.
pub(super) fn write_notes_quote(c: &mut Conv<'_>, lines: Vec<String>) {
    if lines.is_empty() {
        return;
    }
    let mut quote = format!("> **{}**", tr(c.opts.lang, Msg::SlideNotesLabel));
    for l in lines {
        quote.push_str("\n> ");
        quote.push_str(&l.replace('\n', "\n> "));
    }
    c.write_block(Blk::Para(quote));
}

/// What a layout / master gives its placeholders.
fn part_info(bytes: &[u8], root: &str, opts: &DocOptions) -> Result<PartInfo, OfficeError> {
    let mut budget = Budget::new(opts.max_block_nodes, opts.max_block_text_bytes);
    let mut phs: Vec<PhInfo> = Vec::new();
    let mut styles: Option<[Levels; 3]> = None;
    walk_part(
        bytes,
        root,
        &mut budget,
        &mut |n| {
            if phs.len() < 200 {
                if let Some(p) = ph_info(&n) {
                    phs.push(p);
                }
            }
        },
        &mut |n| {
            if n.name == "txStyles" {
                styles = Some(tx_styles(&n));
            }
        },
        &["txStyles"],
    )?;
    Ok(PartInfo {
        phs,
        styles: styles.unwrap_or_default(),
    })
}

/// Moves the whitespace at the edges of a link's text outside the link (the converter trims a link
/// label, which would glue the words around it together).
fn hoist_link_space(segs: &mut Vec<Seg>) {
    let mut i = 0;
    while i < segs.len() {
        if !matches!(segs[i], Seg::LinkOpen(_)) {
            i += 1;
            continue;
        }
        let Some(c) = segs[i..]
            .iter()
            .position(|s| matches!(s, Seg::LinkClose))
            .map(|p| i + p)
        else {
            break;
        };
        // Trailing space first: it is after everything that moves before.
        let mut after = None;
        if c > i + 1 {
            if let Seg::Text(t, _) = &mut segs[c - 1] {
                let keep = t.trim_end().len();
                if keep < t.len() {
                    after = Some(t.split_off(keep));
                }
            }
        }
        if let Some(ws) = after {
            segs.insert(c + 1, Seg::Text(ws, F::default()));
        }
        let mut before = None;
        if c > i + 1 {
            if let Seg::Text(t, _) = &mut segs[i + 1] {
                let cut = t.len() - t.trim_start().len();
                if cut > 0 {
                    before = Some(t.drain(..cut).collect::<String>());
                }
            }
        }
        let mut next = c + 1;
        if let Some(ws) = before {
            segs.insert(i, Seg::Text(ws, F::default()));
            next += 1;
        }
        i = next;
    }
}

/// Whether a paragraph shows anything.
fn visible(inl: &Inl) -> bool {
    inl.segs.iter().any(|s| match s {
        Seg::Text(t, _) => !t.trim().is_empty(),
        Seg::Raw(_) | Seg::Math(_) | Seg::Display(_) => true,
        _ => false,
    })
}

/// The title of a slide as one line of plain text.
fn plain_title(tx: &Node) -> String {
    let mut parts: Vec<String> = Vec::new();
    for p in tx.nodes().filter(|n| n.name == "p") {
        let mut s = String::new();
        for n in p.nodes() {
            match n.name.as_str() {
                "r" | "fld" => collect_t_text(n, &mut s, 0),
                "br" => s.push(' '),
                _ => {}
            }
        }
        parts.push(s);
    }
    let one = clean(&parts.join(" "));
    one.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(TITLE_CHARS)
        .collect()
}

/// The text paragraphs of a diagram shape / data point (`txBody` or `t`).
fn paragraphs_of(tx: &Node) -> Vec<String> {
    let mut out = Vec::new();
    for p in tx.nodes().filter(|n| n.name == "p") {
        let mut s = String::new();
        for n in p.nodes() {
            match n.name.as_str() {
                "r" | "fld" => collect_t_text(n, &mut s, 0),
                "br" => s.push(' '),
                _ => {}
            }
        }
        let s: String = clean(&s)
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(500)
            .collect();
        if !s.is_empty() {
            out.push(s);
        }
    }
    out
}

/// The text of a SmartArt data model (`dgm:pt` of type node) and the relationship id of its
/// drawing (`dsp:dataModelExt`).
fn scan_diagram_data(bytes: &[u8]) -> (Vec<Vec<String>>, Option<String>) {
    let mut rd = XmlReader::new(bytes);
    let mut buf = Vec::new();
    let mut texts: Vec<Vec<String>> = Vec::new();
    let mut drawing = None;
    let mut budget_points = SMART_SHAPES;
    loop {
        buf.clear();
        let Ok(ev) = rd.read_event_into(&mut buf) else {
            break;
        };
        let (e, empty) = match ev {
            Event::Start(e) => (e.into_owned(), false),
            Event::Empty(e) => (e.into_owned(), true),
            Event::Eof => break,
            _ => continue,
        };
        match e.local_name().as_ref() {
            b"dataModelExt" => {
                if drawing.is_none() {
                    drawing = attr(&e, b"relId", false);
                }
            }
            b"pt" if budget_points > 0 => {
                let mut b = Budget::new(SMART_NODES, 256 * 1024);
                match read_element(&mut rd, &e, empty, &mut b) {
                    Ok(Tree::Ok(n)) => {
                        budget_points -= 1;
                        // A data point is a node unless it says otherwise (`pres`, `parTrans` ..).
                        if matches!(n.attr("type"), None | Some("node")) {
                            if let Some(t) = n.child("t") {
                                let ps = paragraphs_of(t);
                                if !ps.is_empty() {
                                    texts.push(ps);
                                }
                            }
                        }
                    }
                    Ok(Tree::TooBig) => budget_points = budget_points.saturating_sub(1),
                    Err(_) => break,
                }
            }
            _ => {}
        }
    }
    (texts, drawing)
}

/// The text shapes (`dsp:sp`) of a SmartArt drawing with their positions.
fn scan_diagram_drawing(bytes: &[u8]) -> Vec<(Option<Rect>, Vec<String>)> {
    let mut rd = XmlReader::new(bytes);
    let mut buf = Vec::new();
    let mut out = Vec::new();
    loop {
        buf.clear();
        let Ok(ev) = rd.read_event_into(&mut buf) else {
            break;
        };
        let (e, empty) = match ev {
            Event::Start(e) => (e.into_owned(), false),
            Event::Empty(e) => (e.into_owned(), true),
            Event::Eof => break,
            _ => continue,
        };
        if e.local_name().as_ref() != b"sp" || out.len() >= SMART_SHAPES {
            continue;
        }
        let mut b = Budget::new(SMART_NODES, 256 * 1024);
        match read_element(&mut rd, &e, empty, &mut b) {
            Ok(Tree::Ok(n)) => {
                let ps = n.child("txBody").map(paragraphs_of).unwrap_or_default();
                if !ps.is_empty() {
                    let rect = n
                        .child("spPr")
                        .and_then(|p| p.child("xfrm"))
                        .and_then(xfrm_rect);
                    out.push((rect, ps));
                }
            }
            Ok(Tree::TooBig) => {}
            Err(_) => break,
        }
    }
    out
}
