//! Tables: a `p:graphicFrame` holding an `a:tbl` becomes cell shapes (fill and text), border lines
//! and diagonal lines of the scene.
//!
//! # Geometry
//!
//! * The table sits at the frame's `p:xfrm` offset. Its columns are the `a:gridCol w` widths and
//!   its rows the `a:tr h` heights -- not the frame's extent, which PowerPoint only keeps in step
//!   with them. A zero-column or zero-row table draws nothing.
//! * A row **grows** to fit its tallest cell: the cell's text is laid out (the same layout the
//!   renderer will use) at the cell's text width and the row becomes at least text height + top
//!   and bottom margin. A cell spanning several rows adds what is missing to the last row it
//!   spans. The table can therefore end below its frame (as in PowerPoint); nothing below it
//!   moves. Cells with vertical text do not grow their row.
//! * Merges: `gridSpan` / `rowSpan` on the origin cell, `hMerge` / `vMerge` on the cells it
//!   covers. The origin cell is drawn over the whole merged box; spans are clamped to the grid and
//!   to the cells already taken by an earlier cell. A cell marked `hMerge` / `vMerge` that no
//!   origin covers is drawn as an ordinary cell without its text.
//! * `a:tblPr rtl="1"` mirrors the columns (the first column is on the right). Borders mirror with
//!   their cells (`lnL` stays the border on the cell's logical left).
//!
//! # Cell properties (`a:tcPr`)
//!
//! `marL` / `marR` / `marT` / `marB` (91 440 / 91 440 / 45 720 / 45 720 EMU), `anchor`,
//! `anchorCtr`, `vert`, the fill (`noFill` is an explicit "no fill" over the table style), the
//! borders `lnL` / `lnR` / `lnT` / `lnB` and the diagonals `lnTlToBr` / `lnBlToTr`. `horzOverflow`
//! is not used: the renderer never clips text. A border element with no attribute and no child is
//! "not set" (the style decides); one that sets no visible fill (`a:noFill`) removes the border.
//! A border with a fill but no `w` is 1 pt.
//!
//! # Table styles
//!
//! `a:tblPr` names a style with `a:tableStyleId` (looked up in `ppt/tableStyles.xml`) or carries
//! one inline (`a:tblStyle` / `a:tableStyle`). Not found: the built-in "Medium Style 2" family
//! ([`BUILTIN_MEDIUM_STYLE_2`]: the seven ids PowerPoint writes for it without a definition, e.g.
//! its default `{5C22544A-...}` "Medium Style 2 - Accent 1"), whose definition is the one
//! PowerPoint stores in a deck; any other missing id draws no style at all. Without a style id
//! there is no style either.
//!
//! The parts of the style that apply to a cell, lowest precedence first: `wholeTbl`; `band1V` /
//! `band2V` (`bandCol`, not on the first / last column); `band1H` / `band2H` (`bandRow`, not on the
//! first / last row); `firstCol`, `lastCol`; `firstRow`, `lastRow`; `nwCell`, `neCell`, `swCell`,
//! `seCell` (both flags of the corner). Bands count from the first row / column after the header
//! one. A later part overrides an earlier one property by property: the fill, each border, bold,
//! italic, the text colour, the font. A direct `a:tcPr` fill overrides all of them; `a:tblBg`
//! is the fill below `wholeTbl`.
//!
//! A style part's `left` / `right` / `top` / `bottom` borders are the edges of the part's region
//! (the whole table; the row for `firstRow`, the band rows and `lastRow`; the column for the
//! column parts; the cell for a corner), `insideH` / `insideV` the lines between the cells within
//! it: a cell takes `left` where it is on the region's left edge and `insideV` elsewhere, and so
//! on. A border is a `ln` or a `lnRef` into the theme's line list.
//!
//! The text of a cell takes the table style's `tcTxStyle` (bold, italic, colour, theme font) as its
//! lowest list style, then the master's `otherStyle` and the presentation's default text style --
//! so the font size is the default one (18 pt) unless the runs say otherwise.
//!
//! # Borders on shared edges
//!
//! Every edge between two cells has up to two candidates (the cell above's / left's and the cell
//! below's / right's). A direct `tcPr` border beats any style border; among direct borders the
//! later cell's (below / right) wins; between two style borders the one from the part with the
//! higher precedence wins (so a header row's thick bottom line beats the body's thin `insideH`),
//! the later cell's on a tie. A candidate that is "no line" wins like any other. Edges inside a
//! merged cell are not drawn; along its outline the borders of the cells it covers count too.
//! Equal neighbouring segments are drawn as one line, with square caps so that corners close.
//!
//! # Budgets
//!
//! [`MAX_TABLE_ROWS`], [`MAX_TABLE_COLS`], [`MAX_TABLE_CELLS`] (rows x columns), [`MAX_CELL_CHARS`]
//! text characters in a cell, the items of the slide (`DocOptions::max_slide_shapes`), the styles
//! kept of `tableStyles.xml` ([`MAX_TABLE_STYLES`]). Going over one drops the rest and sets
//! `truncated`.

use quick_xml::events::BytesStart;

use crate::preview::office::slide_draw as sd;
use crate::preview::office::slide_draw::text as sdt;

use super::style::{flag, num, FillRes, LineSpec};
use super::text::TextChain;
use super::*;

#[cfg(test)]
mod tests;

/// Most rows of a table that are drawn.
pub(super) const MAX_TABLE_ROWS: usize = 500;
/// Most columns of a table that are drawn.
pub(super) const MAX_TABLE_COLS: usize = 100;
/// Most cells (rows x columns) of a table that are drawn; rows past it are dropped.
pub(super) const MAX_TABLE_CELLS: usize = 5_000;
/// Most text characters kept of one cell.
pub(super) const MAX_CELL_CHARS: usize = 10_000;
/// Most styles kept of `ppt/tableStyles.xml`.
pub(super) const MAX_TABLE_STYLES: usize = 128;

/// Width of a border that sets a fill but no width (1 pt).
const DEFAULT_BORDER_W: f64 = 12_700.0;
/// The precedence of a direct `tcPr` border (above every style part).
const EXPLICIT_RANK: u8 = 100;
/// Largest length / size read from a table (EMU).
const MAX_EMU: f64 = 4.0e9;

/// The ids PowerPoint writes for "Medium Style 2" without storing a definition, with the theme
/// colour each uses: `{5C22544A-...}` is Accent 1, `{073A0DAA-...}` the one in the text colour.
const BUILTIN_MEDIUM_STYLE_2: [(&str, &str); 7] = [
    ("{5C22544A-7EE6-4342-B048-85BDC9FD1C3A}", "accent1"),
    ("{21E4AEA4-8DFA-4A89-87EB-49C32662AFE0}", "accent2"),
    ("{F5AB1C69-6EDB-4FF4-983F-18BD219EF322}", "accent3"),
    ("{00A15C55-8517-42AA-B614-E9B94910E393}", "accent4"),
    ("{7DF18680-E054-41AD-8BC1-D1AEF772440D}", "accent5"),
    ("{93296810-A885-4BE3-A3E7-6D5BEEA58F35}", "accent6"),
    ("{073A0DAA-6AF3-43AB-8588-CEC1D06C72B9}", "dk1"),
];

/// "Medium Style 2 - Accent 1" as PowerPoint stores it in a deck, with `ACCENT` standing for the
/// theme colour.
const MEDIUM_STYLE_2_XML: &str = r#"<a:tblStyle xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" styleId="x" styleName="Medium Style 2"><a:wholeTbl><a:tcTxStyle><a:fontRef idx="minor"><a:prstClr val="black"/></a:fontRef><a:schemeClr val="dk1"/></a:tcTxStyle><a:tcStyle><a:tcBdr><a:left><a:ln w="12700" cmpd="sng"><a:solidFill><a:schemeClr val="lt1"/></a:solidFill></a:ln></a:left><a:right><a:ln w="12700" cmpd="sng"><a:solidFill><a:schemeClr val="lt1"/></a:solidFill></a:ln></a:right><a:top><a:ln w="12700" cmpd="sng"><a:solidFill><a:schemeClr val="lt1"/></a:solidFill></a:ln></a:top><a:bottom><a:ln w="12700" cmpd="sng"><a:solidFill><a:schemeClr val="lt1"/></a:solidFill></a:ln></a:bottom><a:insideH><a:ln w="12700" cmpd="sng"><a:solidFill><a:schemeClr val="lt1"/></a:solidFill></a:ln></a:insideH><a:insideV><a:ln w="12700" cmpd="sng"><a:solidFill><a:schemeClr val="lt1"/></a:solidFill></a:ln></a:insideV></a:tcBdr><a:fill><a:solidFill><a:schemeClr val="ACCENT"><a:tint val="20000"/></a:schemeClr></a:solidFill></a:fill></a:tcStyle></a:wholeTbl><a:band1H><a:tcStyle><a:tcBdr/><a:fill><a:solidFill><a:schemeClr val="ACCENT"><a:tint val="40000"/></a:schemeClr></a:solidFill></a:fill></a:tcStyle></a:band1H><a:band2H><a:tcStyle><a:tcBdr/></a:tcStyle></a:band2H><a:band1V><a:tcStyle><a:tcBdr/><a:fill><a:solidFill><a:schemeClr val="ACCENT"><a:tint val="40000"/></a:schemeClr></a:solidFill></a:fill></a:tcStyle></a:band1V><a:band2V><a:tcStyle><a:tcBdr/></a:tcStyle></a:band2V><a:lastCol><a:tcTxStyle b="on"><a:fontRef idx="minor"><a:prstClr val="black"/></a:fontRef><a:schemeClr val="lt1"/></a:tcTxStyle><a:tcStyle><a:tcBdr/><a:fill><a:solidFill><a:schemeClr val="ACCENT"/></a:solidFill></a:fill></a:tcStyle></a:lastCol><a:firstCol><a:tcTxStyle b="on"><a:fontRef idx="minor"><a:prstClr val="black"/></a:fontRef><a:schemeClr val="lt1"/></a:tcTxStyle><a:tcStyle><a:tcBdr/><a:fill><a:solidFill><a:schemeClr val="ACCENT"/></a:solidFill></a:fill></a:tcStyle></a:firstCol><a:lastRow><a:tcTxStyle b="on"><a:fontRef idx="minor"><a:prstClr val="black"/></a:fontRef><a:schemeClr val="lt1"/></a:tcTxStyle><a:tcStyle><a:tcBdr><a:top><a:ln w="38100" cmpd="sng"><a:solidFill><a:schemeClr val="lt1"/></a:solidFill></a:ln></a:top></a:tcBdr><a:fill><a:solidFill><a:schemeClr val="ACCENT"/></a:solidFill></a:fill></a:tcStyle></a:lastRow><a:firstRow><a:tcTxStyle b="on"><a:fontRef idx="minor"><a:prstClr val="black"/></a:fontRef><a:schemeClr val="lt1"/></a:tcTxStyle><a:tcStyle><a:tcBdr><a:bottom><a:ln w="38100" cmpd="sng"><a:solidFill><a:schemeClr val="lt1"/></a:solidFill></a:ln></a:bottom></a:tcBdr><a:fill><a:solidFill><a:schemeClr val="ACCENT"/></a:solidFill></a:fill></a:tcStyle></a:firstRow></a:tblStyle>"#;

// ---------------------------------------------------------------------------------------------
// ppt/tableStyles.xml
// ---------------------------------------------------------------------------------------------

/// The table styles of a deck (`ppt/tableStyles.xml`): the `a:tblStyle` elements by `styleId`.
#[derive(Debug, Default)]
pub(in super::super) struct TableStyles {
    styles: Vec<(String, Node)>,
    /// The part held more styles (or nodes) than are kept.
    truncated: bool,
}

/// Reads one element that starts at `e` out of `rd`; `None` over the budget.
fn read_node(
    rd: &mut XmlReader<&[u8]>,
    e: &BytesStart<'_>,
    empty: bool,
    budget: &mut Budget,
) -> Option<Node> {
    match read_element(rd, e, empty, budget) {
        Ok(Tree::Ok(n)) => Some(n),
        _ => None,
    }
}

impl TableStyles {
    /// Reads `tableStyles.xml`. A damaged part keeps the styles read before the damage.
    pub(in super::super) fn parse(bytes: &[u8], opts: &DocOptions) -> TableStyles {
        let mut out = TableStyles::default();
        let mut rd = XmlReader::new(bytes);
        let mut buf = Vec::new();
        let mut budget = Budget::new(opts.max_block_nodes.min(100_000), 1 << 20);
        loop {
            buf.clear();
            let (e, empty) = match rd.read_event_into(&mut buf) {
                Ok(Event::Start(e)) => (e.into_owned(), false),
                Ok(Event::Empty(e)) => (e.into_owned(), true),
                Ok(Event::Eof) | Err(_) => break,
                _ => continue,
            };
            if e.local_name().as_ref() != b"tblStyle" {
                continue;
            }
            if out.styles.len() >= MAX_TABLE_STYLES {
                out.truncated = true;
                break;
            }
            let Some(n) = read_node(&mut rd, &e, empty, &mut budget) else {
                out.truncated = true;
                break;
            };
            if let Some(id) = n.attr("styleId") {
                let id = id.trim().to_string();
                out.styles.push((id, n));
            }
        }
        out
    }

    fn get(&self, id: &str) -> Option<&Node> {
        self.styles.iter().find(|(k, _)| k == id).map(|(_, n)| n)
    }
}

/// The built-in definition of a style id PowerPoint knows by heart (see
/// [`BUILTIN_MEDIUM_STYLE_2`]).
fn builtin_style(id: &str) -> Option<Node> {
    let (_, accent) = BUILTIN_MEDIUM_STYLE_2
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(id))?;
    let xml = MEDIUM_STYLE_2_XML.replace("ACCENT", accent);
    let mut rd = XmlReader::new(xml.as_bytes());
    let mut buf = Vec::new();
    let mut budget = Budget::new(10_000, 1 << 16);
    loop {
        buf.clear();
        let (e, empty) = match rd.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => (e.into_owned(), false),
            Ok(Event::Empty(e)) => (e.into_owned(), true),
            Ok(Event::Eof) | Err(_) => return None,
            _ => continue,
        };
        return read_node(&mut rd, &e, empty, &mut budget);
    }
}

// ---------------------------------------------------------------------------------------------
// the style of a table
// ---------------------------------------------------------------------------------------------

/// The parts of a table style, lowest precedence first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Part {
    Whole,
    Band1V,
    Band2V,
    Band1H,
    Band2H,
    FirstCol,
    LastCol,
    FirstRow,
    LastRow,
    NwCell,
    NeCell,
    SwCell,
    SeCell,
}

const PARTS: [(Part, &str); 13] = [
    (Part::Whole, "wholeTbl"),
    (Part::Band1V, "band1V"),
    (Part::Band2V, "band2V"),
    (Part::Band1H, "band1H"),
    (Part::Band2H, "band2H"),
    (Part::FirstCol, "firstCol"),
    (Part::LastCol, "lastCol"),
    (Part::FirstRow, "firstRow"),
    (Part::LastRow, "lastRow"),
    (Part::NwCell, "nwCell"),
    (Part::NeCell, "neCell"),
    (Part::SwCell, "swCell"),
    (Part::SeCell, "seCell"),
];

impl Part {
    /// How strongly the part's borders count against another part's on a shared edge.
    fn rank(self) -> u8 {
        match self {
            Part::Whole => 0,
            Part::Band1V | Part::Band2V => 1,
            Part::Band1H | Part::Band2H => 2,
            Part::FirstCol | Part::LastCol => 3,
            Part::FirstRow | Part::LastRow => 4,
            _ => 5,
        }
    }

    /// Which of the region's edges the cell at (`r`, `c`) is on: (left, right, top, bottom).
    fn region(self, r: usize, c: usize, nr: usize, nc: usize) -> (bool, bool, bool, bool) {
        match self {
            Part::Whole => (c == 0, c + 1 == nc, r == 0, r + 1 == nr),
            Part::Band1H | Part::Band2H | Part::FirstRow | Part::LastRow => {
                (c == 0, c + 1 == nc, true, true)
            }
            Part::Band1V | Part::Band2V | Part::FirstCol | Part::LastCol => {
                (true, true, r == 0, r + 1 == nr)
            }
            _ => (true, true, true, true),
        }
    }
}

/// Edge slots of a style part / a cell: the four sides, the lines between cells of the region,
/// and the two diagonals.
const E_L: usize = 0;
const E_R: usize = 1;
const E_T: usize = 2;
const E_B: usize = 3;
const E_IH: usize = 4;
const E_IV: usize = 5;
const E_TLBR: usize = 6;
const E_TRBL: usize = 7;
const EDGE_NAMES: [&str; 8] = [
    "left", "right", "top", "bottom", "insideH", "insideV", "tl2br", "tr2bl",
];

/// What a border element says: unset (`None`), "no line" (`Some(None)`) or a line.
type EdgeSpec = Option<Option<sd::Line>>;

/// What one part of a table style sets.
#[derive(Debug, Clone, Default)]
struct PartRes {
    fill: Option<sd::Fill>,
    edges: [EdgeSpec; 8],
    bold: Option<bool>,
    italic: Option<bool>,
    /// The colour element of `tcTxStyle` (resolved with the text, with its transforms).
    color: Option<Node>,
    font: Option<&'static str>,
}

/// A whole table style, resolved.
#[derive(Debug, Default)]
struct StyleSet {
    parts: Vec<PartRes>,
    /// `a:tblBg`: the fill below `wholeTbl`.
    bg: Option<sd::Fill>,
}

/// The text properties a style gives a cell.
#[derive(Debug, Clone, Default)]
struct TxProps {
    bold: Option<bool>,
    italic: Option<bool>,
    color: Option<Node>,
    font: Option<&'static str>,
}

/// What a style gives one grid cell.
#[derive(Debug, Default)]
struct CellStyle {
    fill: Option<sd::Fill>,
    /// (rank of the part, line) for left, right, top, bottom, then the two diagonals.
    edges: [Option<(u8, Option<sd::Line>)>; 6],
    tx: TxProps,
}

/// The `a:tblPr` flags.
#[derive(Debug, Clone, Copy, Default)]
struct Flags {
    first_row: bool,
    last_row: bool,
    first_col: bool,
    last_col: bool,
    band_row: bool,
    band_col: bool,
}

impl Flags {
    /// The parts of the style that apply to the cell at (`r`, `c`), lowest precedence first.
    fn parts(&self, r: usize, c: usize, nr: usize, nc: usize) -> Vec<Part> {
        let (fr, lr) = (self.first_row && r == 0, self.last_row && r + 1 == nr);
        let (fc, lc) = (self.first_col && c == 0, self.last_col && c + 1 == nc);
        let mut v = vec![Part::Whole];
        if self.band_col && !fc && !lc {
            let k = c - usize::from(self.first_col);
            v.push(if k.is_multiple_of(2) {
                Part::Band1V
            } else {
                Part::Band2V
            });
        }
        if self.band_row && !fr && !lr {
            let k = r - usize::from(self.first_row);
            v.push(if k.is_multiple_of(2) {
                Part::Band1H
            } else {
                Part::Band2H
            });
        }
        if fc {
            v.push(Part::FirstCol);
        }
        if lc {
            v.push(Part::LastCol);
        }
        if fr {
            v.push(Part::FirstRow);
        }
        if lr {
            v.push(Part::LastRow);
        }
        if fr && fc {
            v.push(Part::NwCell);
        }
        if fr && lc {
            v.push(Part::NeCell);
        }
        if lr && fc {
            v.push(Part::SwCell);
        }
        if lr && lc {
            v.push(Part::SeCell);
        }
        v
    }
}

impl StyleSet {
    fn of(&self, p: Part) -> &PartRes {
        &self.parts[p as usize]
    }

    /// The style of the cell at (`r`, `c`) (see the module documentation).
    fn cell(&self, f: &Flags, r: usize, c: usize, nr: usize, nc: usize) -> CellStyle {
        let mut cs = CellStyle {
            fill: self.bg.clone(),
            ..CellStyle::default()
        };
        for part in f.parts(r, c, nr, nc) {
            let p = self.of(part);
            let rank = part.rank();
            if p.fill.is_some() {
                cs.fill = p.fill.clone();
            }
            let (al, ar, at, ab) = part.region(r, c, nr, nc);
            let pick = |outer: bool, side: usize, inside: usize| {
                if outer {
                    p.edges[side].clone()
                } else {
                    p.edges[inside].clone()
                }
            };
            let edges = [
                pick(al, E_L, E_IV),
                pick(ar, E_R, E_IV),
                pick(at, E_T, E_IH),
                pick(ab, E_B, E_IH),
                p.edges[E_TLBR].clone(),
                p.edges[E_TRBL].clone(),
            ];
            for (slot, e) in cs.edges.iter_mut().zip(edges) {
                if let Some(line) = e {
                    *slot = Some((rank, line));
                }
            }
            if p.bold.is_some() {
                cs.tx.bold = p.bold;
            }
            if p.italic.is_some() {
                cs.tx.italic = p.italic;
            }
            if p.color.is_some() {
                cs.tx.color = p.color.clone();
            }
            if p.font.is_some() {
                cs.tx.font = p.font;
            }
        }
        cs
    }
}

fn onoff(v: Option<&str>) -> Option<bool> {
    match v?.trim() {
        "on" | "1" | "true" => Some(true),
        "off" | "0" | "false" => Some(false),
        _ => None,
    }
}

fn synth(name: &str, attrs: Vec<(String, String)>, kids: Vec<Node>) -> Node {
    Node {
        prefix: "a".into(),
        name: name.into(),
        attrs,
        kids: kids.into_iter().map(Kid::N).collect(),
    }
}

impl TxProps {
    /// The properties as the lowest list style of the cell's text (`lvl1pPr/defRPr`), if any is set.
    fn list_style(&self) -> Option<Node> {
        if self.bold.is_none()
            && self.italic.is_none()
            && self.color.is_none()
            && self.font.is_none()
        {
            return None;
        }
        let flag = |v: bool| (if v { "1" } else { "0" }).to_string();
        let mut attrs = Vec::new();
        if let Some(b) = self.bold {
            attrs.push(("b".to_string(), flag(b)));
        }
        if let Some(i) = self.italic {
            attrs.push(("i".to_string(), flag(i)));
        }
        let mut kids = Vec::new();
        if let Some(c) = &self.color {
            kids.push(synth("solidFill", Vec::new(), vec![c.clone()]));
        }
        if let Some(f) = self.font {
            kids.push(synth(
                "latin",
                vec![("typeface".to_string(), f.to_string())],
                Vec::new(),
            ));
        }
        Some(synth(
            "lvl1pPr",
            Vec::new(),
            vec![synth("defRPr", attrs, kids)],
        ))
    }
}

/// A line to draw from a spec: a fill but no width is 1 pt.
fn border_line(mut spec: LineSpec) -> Option<sd::Line> {
    if spec.width.is_none() {
        spec.width = Some(DEFAULT_BORDER_W);
    }
    spec.finish()
}

impl Sb<'_> {
    /// One part of a style (`wholeTbl` ..): its text style, borders and fill.
    fn part_res(&mut self, st: &Node, name: &str) -> PartRes {
        let mut r = PartRes::default();
        let Some(p) = st.child(name) else {
            return r;
        };
        let no_rels = HashMap::new();
        if let Some(tx) = p.child("tcTxStyle") {
            r.bold = onoff(tx.attr("b"));
            r.italic = onoff(tx.attr("i"));
            let is_clr = |c: &&Node| theme::is_color_elem(&c.name);
            r.color = tx
                .nodes()
                .find(is_clr)
                .or_else(|| tx.child("fontRef").and_then(|f| f.nodes().find(is_clr)))
                .cloned();
            r.font = match tx.child("fontRef").and_then(|f| f.attr("idx")) {
                Some("major") => Some("+mj-lt"),
                Some("minor") => Some("+mn-lt"),
                _ => None,
            };
        }
        if let Some(ts) = p.child("tcStyle") {
            if let Some(b) = ts.child("tcBdr") {
                for (i, n) in EDGE_NAMES.iter().enumerate() {
                    let Some(e) = b.child(n) else { continue };
                    r.edges[i] = if let Some(ln) = e.child("ln") {
                        Some(border_line(self.line_spec(ln, None, &no_rels)))
                    } else if e.child("lnRef").is_some() {
                        Some(border_line(self.style_line(e)))
                    } else {
                        None
                    };
                }
            }
            if let Some(f) = ts.child("fill") {
                if let Some(FillRes::Set(fl)) = self.fill_in(f, None, &no_rels) {
                    r.fill = Some(fl);
                }
            } else if let Some(fr) = ts.child("fillRef") {
                let ph = self.col.first_in(fr, None);
                let idx = num(fr, "idx").unwrap_or(0.0) as usize;
                r.fill = self.theme_fill(idx, ph);
            }
        }
        r
    }

    /// All parts of a style (an absent style sets nothing).
    fn style_set(&mut self, st: Option<&Node>) -> StyleSet {
        let Some(st) = st else {
            return StyleSet {
                parts: vec![PartRes::default(); PARTS.len()],
                bg: None,
            };
        };
        let parts = PARTS
            .iter()
            .map(|(_, name)| self.part_res(st, name))
            .collect();
        let no_rels = HashMap::new();
        let bg = st.child("tblBg").and_then(|b| {
            if let Some(FillRes::Set(f)) = self.fill_in(b, None, &no_rels) {
                return Some(f);
            }
            let fr = b.child("fillRef")?;
            let ph = self.col.first_in(fr, None);
            self.theme_fill(num(fr, "idx").unwrap_or(0.0) as usize, ph)
        });
        StyleSet { parts, bg }
    }

    /// What a direct `tcPr` says about its borders: left, right, top, bottom, the two diagonals.
    fn explicit_edges(&mut self, tc_pr: &Node) -> [EdgeSpec; 6] {
        let rels = self.rels[self.cur.min(2)];
        let mut out: [EdgeSpec; 6] = Default::default();
        for (slot, name) in out
            .iter_mut()
            .zip(["lnL", "lnR", "lnT", "lnB", "lnTlToBr", "lnBlToTr"])
        {
            let Some(e) = tc_pr.child(name) else { continue };
            if e.attrs.is_empty() && e.kids.is_empty() {
                continue;
            }
            *slot = Some(border_line(self.line_spec(e, None, rels)));
        }
        out
    }

    /// Spends one item of the slide's budget; `false` (and `truncated`) when it is used up.
    fn table_item(&mut self, out: &mut Vec<sd::Item>, item: sd::Item) -> bool {
        if self.items >= self.max_items {
            self.truncated = true;
            return false;
        }
        self.items += 1;
        out.push(item);
        true
    }
}

// ---------------------------------------------------------------------------------------------
// the grid
// ---------------------------------------------------------------------------------------------

/// A cell that owns a box of the grid.
struct Cell<'n> {
    node: &'n Node,
    r: usize,
    c: usize,
    rs: usize,
    cs: usize,
    /// A `hMerge` / `vMerge` cell nothing covers: drawn without its text.
    orphan: bool,
    body: Option<sd::TextBody>,
    need: f64,
}

const NONE: usize = usize::MAX;

/// The parsed grid: columns, rows, the cells and which cell owns each slot.
struct Grid<'n> {
    widths: Vec<f64>,
    heights: Vec<f64>,
    /// `tc` at each slot (row-major), if the row has one there.
    slots: Vec<Option<&'n Node>>,
    owner: Vec<usize>,
    cells: Vec<Cell<'n>>,
    truncated: bool,
}

fn emu(v: Option<f64>) -> f64 {
    v.map_or(0.0, |v| v.clamp(0.0, MAX_EMU))
}

impl<'n> Grid<'n> {
    fn parse(tbl: &'n Node) -> Option<Grid<'n>> {
        let mut truncated = false;
        let mut widths: Vec<f64> = tbl
            .child("tblGrid")
            .map(|g| {
                g.nodes()
                    .filter(|n| n.name == "gridCol")
                    .take(MAX_TABLE_COLS + 1)
                    .map(|n| emu(num(n, "w")))
                    .collect()
            })
            .unwrap_or_default();
        if widths.len() > MAX_TABLE_COLS {
            widths.truncate(MAX_TABLE_COLS);
            truncated = true;
        }
        let nc = widths.len();
        if nc == 0 {
            return None;
        }
        let max_rows = MAX_TABLE_ROWS.min(MAX_TABLE_CELLS / nc);
        let mut trs: Vec<&Node> = tbl
            .nodes()
            .filter(|n| n.name == "tr")
            .take(max_rows + 1)
            .collect();
        if trs.len() > max_rows {
            trs.truncate(max_rows);
            truncated = true;
        }
        let nr = trs.len();
        if nr == 0 {
            return None;
        }
        let heights: Vec<f64> = trs.iter().map(|t| emu(num(t, "h"))).collect();
        let mut slots: Vec<Option<&Node>> = vec![None; nr * nc];
        for (r, tr) in trs.iter().enumerate() {
            for (c, tc) in tr.nodes().filter(|n| n.name == "tc").enumerate() {
                if c >= nc {
                    truncated = true;
                    break;
                }
                slots[r * nc + c] = Some(tc);
            }
        }
        let mut g = Grid {
            widths,
            heights,
            slots,
            owner: vec![NONE; nr * nc],
            cells: Vec::new(),
            truncated,
        };
        g.place();
        Some(g)
    }

    fn nr(&self) -> usize {
        self.heights.len()
    }

    fn nc(&self) -> usize {
        self.widths.len()
    }

    /// Finds the cells: every slot that no earlier cell covers and that has a `tc` starts one,
    /// with its spans clamped to the grid and to the free slots.
    fn place(&mut self) {
        let (nr, nc) = (self.nr(), self.nc());
        for r in 0..nr {
            for c in 0..nc {
                let slot = r * nc + c;
                let (Some(tc), NONE) = (self.slots[slot], self.owner[slot]) else {
                    continue;
                };
                let idx = self.cells.len();
                let orphan =
                    flag(tc, "hMerge").unwrap_or(false) || flag(tc, "vMerge").unwrap_or(false);
                let span = |k: &str| {
                    if orphan {
                        1
                    } else {
                        num(tc, k).map_or(1, |v| v.clamp(1.0, 1.0e6) as usize)
                    }
                };
                let mut cs = span("gridSpan").min(nc - c);
                while cs > 1 && !self.free(r, c, cs) {
                    cs -= 1;
                }
                let mut rs = 1;
                let want = span("rowSpan").min(nr - r);
                while rs < want && self.free(r + rs, c, cs) {
                    rs += 1;
                }
                for rr in r..r + rs {
                    for cc in c..c + cs {
                        self.owner[rr * nc + cc] = idx;
                    }
                }
                self.cells.push(Cell {
                    node: tc,
                    r,
                    c,
                    rs,
                    cs,
                    orphan,
                    body: None,
                    need: 0.0,
                });
            }
        }
    }

    /// Whether the slots `c..c + cs` of row `r` are all free of cells.
    fn free(&self, r: usize, c: usize, cs: usize) -> bool {
        let nc = self.nc();
        (c..c + cs).all(|cc| self.owner[r * nc + cc] == NONE)
    }

    /// Lets the rows grow to what their cells need (see the module documentation).
    fn grow_rows(&mut self) {
        let mut order: Vec<usize> = (0..self.cells.len()).collect();
        order.sort_by_key(|&i| self.cells[i].rs);
        for i in order {
            let (r, rs, need) = {
                let c = &self.cells[i];
                (c.r, c.rs, c.need)
            };
            let have: f64 = self.heights[r..r + rs].iter().sum();
            if need > have {
                self.heights[r + rs - 1] += need - have;
            }
        }
    }
}

/// Drops the characters of a body past [`MAX_CELL_CHARS`]; whether any were dropped.
fn cap_cell_text(body: &mut sd::TextBody) -> bool {
    let mut left = MAX_CELL_CHARS;
    let mut cut = false;
    let mut keep_paras = body.paragraphs.len();
    for (pi, p) in body.paragraphs.iter_mut().enumerate() {
        let mut keep_runs = p.runs.len();
        for (ri, run) in p.runs.iter_mut().enumerate() {
            let n = run.text.chars().count();
            if n <= left {
                left -= n;
                continue;
            }
            run.text = run.text.chars().take(left).collect();
            left = 0;
            cut = true;
            keep_runs = ri + 1;
            break;
        }
        p.runs.truncate(keep_runs);
        if left == 0 && cut {
            keep_paras = pi + 1;
            break;
        }
    }
    if cut {
        body.paragraphs.truncate(keep_paras);
    }
    cut
}

/// A border candidate on an edge: the rank of where it comes from and the line (`None`: no line).
type Cand = (u8, Option<sd::Line>);

/// Decides the line on an edge between two cells (see "Borders on shared edges").
fn resolve(a: Option<&Cand>, b: Option<&Cand>) -> Option<sd::Line> {
    match (a, b) {
        (None, None) => None,
        (Some(x), None) | (None, Some(x)) => x.1.clone(),
        (Some(x), Some(y)) => {
            if x.0 > y.0 {
                x.1.clone()
            } else {
                y.1.clone()
            }
        }
    }
}

impl Sb<'_> {
    /// The text of a cell as a body with the cell's margins, anchor and direction, and whether any
    /// of it is visible.
    fn cell_body(&mut self, tc: &Node, style: &CellStyle) -> Option<(sd::TextBody, bool)> {
        let tx = tc.child("txBody")?;
        let chain = TextChain {
            body_pr: tx.child("bodyPr").into_iter().collect(),
            lst: [tx.child("lstStyle"), None, None],
            style: 2,
            font_ref: style.tx.list_style(),
        };
        let (mut body, shown) = self.text_body_raw(tx, &chain);
        if cap_cell_text(&mut body) {
            self.truncated = true;
        }
        let pr = tc.child("tcPr");
        let attr = |k: &str| pr.and_then(|p| p.attr(k));
        let mar = |k: &str, d: f64| {
            pr.and_then(|p| num(p, k))
                .map_or(d, |v| v.clamp(0.0, 1.0e9))
        };
        body.insets = (
            mar("marL", 91_440.0),
            mar("marT", 45_720.0),
            mar("marR", 91_440.0),
            mar("marB", 45_720.0),
        );
        body.anchor = match attr("anchor") {
            Some("ctr") => sd::Anchor::Middle,
            Some("b") => sd::Anchor::Bottom,
            _ => sd::Anchor::Top,
        };
        body.anchor_ctr = attr("anchorCtr").is_some_and(|v| matches!(v.trim(), "1" | "true"));
        body.vert = match attr("vert") {
            Some("vert" | "mongolianVert") => sd::Vert::Vert,
            Some("vert270") => sd::Vert::Vert270,
            Some("eaVert" | "wordArtVert" | "wordArtVertRtl") => sd::Vert::EaVert,
            _ => sd::Vert::Horz,
        };
        body.wrap = true;
        body.autofit = sd::AutoFit::None;
        body.columns = 1;
        body.rot_deg = 0.0;
        body.upright = false;
        Some((body, shown))
    }

    /// The style a table names: from `ppt/tableStyles.xml`, inline, or built in.
    fn table_style_node(&mut self, tp: Option<&Node>) -> Option<Node> {
        let tp = tp?;
        if let Some(inline) = tp.child("tblStyle").or_else(|| tp.child("tableStyle")) {
            return Some(inline.clone());
        }
        let id = tp.child("tableStyleId")?.text();
        let id = id.trim();
        if id.is_empty() {
            return None;
        }
        if let Some(ts) = self.table_styles {
            if let Some(n) = ts.get(id) {
                return Some(n.clone());
            }
            if ts.truncated {
                self.truncated = true;
            }
        }
        builtin_style(id)
    }

    /// A table as scene items (see the module documentation).
    pub(super) fn table_items(&mut self, frame: &Node, xfrm: sd::Xfrm, out: &mut Vec<sd::Item>) {
        let Some(tbl) = frame
            .child("graphic")
            .and_then(|g| g.child("graphicData"))
            .and_then(|d| d.child("tbl"))
        else {
            return;
        };
        let Some(mut grid) = Grid::parse(tbl) else {
            return;
        };
        let (nr, nc) = (grid.nr(), grid.nc());
        if grid.truncated {
            self.truncated = true;
        }
        let tp = tbl.child("tblPr");
        let f = |k: &str| tp.and_then(|p| flag(p, k)).unwrap_or(false);
        let flags = Flags {
            first_row: f("firstRow"),
            last_row: f("lastRow"),
            first_col: f("firstCol"),
            last_col: f("lastCol"),
            band_row: f("bandRow"),
            band_col: f("bandCol"),
        };
        let rtl = f("rtl");
        let style_node = self.table_style_node(tp);
        let set = self.style_set(style_node.as_ref());

        // The style of every slot, and the text of every cell with the height it asks for.
        let styles: Vec<CellStyle> = (0..nr * nc)
            .map(|s| set.cell(&flags, s / nc, s % nc, nr, nc))
            .collect();
        for i in 0..grid.cells.len() {
            let (node, r, c, cs, orphan) = {
                let k = &grid.cells[i];
                (k.node, k.r, k.c, k.cs, k.orphan)
            };
            if orphan {
                continue;
            }
            let Some((body, shown)) = self.cell_body(node, &styles[r * nc + c]) else {
                continue;
            };
            let width: f64 = grid.widths[c..c + cs].iter().sum();
            let (l, t, rr, b) = body.insets;
            let mut need = 0.0;
            if body.vert == sd::Vert::Horz && !body.paragraphs.is_empty() {
                let lay = sdt::layout(&body, ((width - l - rr).max(0.0)) / sd::EMU_PER_PX, 0.0);
                if lay.truncated {
                    self.truncated = true;
                }
                need = t + b + lay.content_h * sd::EMU_PER_PX;
            }
            let k = &mut grid.cells[i];
            k.need = need;
            k.body = shown.then_some(body);
        }
        grid.grow_rows();

        // Positions (in the table's own left-to-right order; mirrored below for `rtl`).
        let mut xs = vec![0.0; nc + 1];
        for c in 0..nc {
            xs[c + 1] = xs[c] + grid.widths[c];
        }
        let mut ys = vec![0.0; nr + 1];
        for r in 0..nr {
            ys[r + 1] = ys[r] + grid.heights[r];
        }
        let total_w = xs[nc];
        let px = |p: f64| xfrm.x + if rtl { total_w - p } else { p };
        let py = |p: f64| xfrm.y + p;
        // The box of a run of columns `c0..c1` as (x, width).
        let span_x = |c0: usize, c1: usize| {
            let (a, b) = (px(xs[c0]), px(xs[c1]));
            (a.min(b), (b - a).abs())
        };

        // Direct border properties of every slot's `tc`.
        let mut explicit: Vec<[EdgeSpec; 6]> = Vec::with_capacity(nr * nc);
        let mut fills: Vec<Option<sd::Fill>> = vec![None; grid.cells.len()];
        for s in 0..nr * nc {
            let pr = grid.slots[s].and_then(|tc| tc.child("tcPr"));
            explicit.push(pr.map_or_else(Default::default, |p| self.explicit_edges(p)));
        }
        let rels = self.rels[self.cur.min(2)];
        for (i, k) in grid.cells.iter().enumerate() {
            fills[i] = k
                .node
                .child("tcPr")
                .and_then(|p| self.fill_in(p, None, rels))
                .and_then(|r| match r {
                    FillRes::Set(f) => Some(f),
                    FillRes::Group => None,
                });
        }

        // The candidate of every slot for each of its four sides and two diagonals.
        let cand = |s: usize, side: usize| -> Option<Cand> {
            let (r, c) = (s / nc, s % nc);
            let o = &grid.cells[grid.owner[s]];
            let on_edge = match side {
                0 => c == o.c,
                1 => c + 1 == o.c + o.cs,
                2 => r == o.r,
                3 => r + 1 == o.r + o.rs,
                _ => true,
            };
            let own = explicit[s][side].clone();
            let owner_slot = o.r * nc + o.c;
            let from_owner = if on_edge && owner_slot != s {
                explicit[owner_slot][side].clone()
            } else {
                None
            };
            if let Some(line) = own.or(from_owner) {
                return Some((EXPLICIT_RANK, line));
            }
            styles[s].edges[side].clone()
        };

        // Fills and text first.
        for (i, k) in grid.cells.iter().enumerate() {
            let st = &styles[k.r * nc + k.c];
            let fill = fills[i]
                .clone()
                .or_else(|| st.fill.clone())
                .unwrap_or_default();
            let text = if k.orphan { None } else { k.body.clone() };
            if !fill.is_visible() && text.is_none() {
                continue;
            }
            let (x, w) = span_x(k.c, k.c + k.cs);
            let y = py(ys[k.r]);
            let h = ys[k.r + k.rs] - ys[k.r];
            let mut shape = sd::ShapeItem::new(sd::Xfrm::rect(x, y, w, h), sd::Geometry::Rect);
            shape.fill = fill;
            shape.text = text;
            if !self.table_item(out, sd::Item::Shape(shape)) {
                return;
            }
        }

        // Horizontal borders, then vertical ones, each as runs of equal neighbouring segments.
        let line_item = |x: f64, y: f64, w: f64, h: f64, line: &sd::Line, flip_v: bool| {
            let mut line = line.clone();
            if line.dash == sd::Dash::Solid && line.cap == sd::Cap::Flat {
                line.cap = sd::Cap::Square;
            }
            let mut shape = sd::ShapeItem::new(
                sd::Xfrm {
                    flip_v,
                    ..sd::Xfrm::rect(x, y, w, h)
                },
                sd::Geometry::Line,
            );
            shape.line = Some(line);
            sd::Item::Shape(shape)
        };
        #[allow(clippy::needless_range_loop)] // (`r` is also the row edge being drawn)
        for r in 0..=nr {
            let mut run: Option<(usize, sd::Line)> = None;
            for c in 0..=nc {
                let line = if c < nc {
                    let above = (r > 0).then(|| r * nc - nc + c);
                    let below = (r < nr).then(|| r * nc + c);
                    let same = matches!((above, below), (Some(a), Some(b)) if grid.owner[a] == grid.owner[b]);
                    if same {
                        None
                    } else {
                        let a = above.and_then(|s| cand(s, 3));
                        let b = below.and_then(|s| cand(s, 2));
                        resolve(a.as_ref(), b.as_ref())
                    }
                } else {
                    None
                };
                match (&mut run, line) {
                    (Some((_, l)), Some(n)) if *l == n => {}
                    (r_, n) => {
                        if let Some((c0, l)) = r_.take() {
                            let (x, w) = span_x(c0, c);
                            if !self.table_item(out, line_item(x, py(ys[r]), w, 0.0, &l, false)) {
                                return;
                            }
                        }
                        run = n.map(|n| (c, n));
                    }
                }
            }
        }
        #[allow(clippy::needless_range_loop)] // (`c` is also the column edge being drawn)
        for c in 0..=nc {
            let mut run: Option<(usize, sd::Line)> = None;
            for r in 0..=nr {
                let line = if r < nr {
                    let left = (c > 0).then(|| r * nc + c - 1);
                    let right = (c < nc).then(|| r * nc + c);
                    let same = matches!((left, right), (Some(a), Some(b)) if grid.owner[a] == grid.owner[b]);
                    if same {
                        None
                    } else {
                        let a = left.and_then(|s| cand(s, 1));
                        let b = right.and_then(|s| cand(s, 0));
                        resolve(a.as_ref(), b.as_ref())
                    }
                } else {
                    None
                };
                match (&mut run, line) {
                    (Some((_, l)), Some(n)) if *l == n => {}
                    (r_, n) => {
                        if let Some((r0, l)) = r_.take() {
                            let x = px(xs[c]);
                            let h = ys[r] - ys[r0];
                            if !self.table_item(out, line_item(x, py(ys[r0]), 0.0, h, &l, false)) {
                                return;
                            }
                        }
                        run = n.map(|n| (r, n));
                    }
                }
            }
        }

        // Diagonals of the cells that own a box.
        for k in &grid.cells {
            let s = k.r * nc + k.c;
            let (x, w) = span_x(k.c, k.c + k.cs);
            let (y, h) = (py(ys[k.r]), ys[k.r + k.rs] - ys[k.r]);
            for (side, flip_v) in [(4usize, false), (5, true)] {
                // (Under `rtl` the diagonals mirror with the cell.)
                let flip_v = flip_v ^ rtl;
                if let Some((_, Some(l))) = cand(s, side) {
                    if !self.table_item(out, line_item(x, y, w, h, &l, flip_v)) {
                        return;
                    }
                }
            }
        }
    }
}
