//! An OpenDocument chart (`Object N/content.xml` of an embedded object: `office:chart` >
//! `chart:chart`) as a [`ch::ChartModel`], which `slide_draw::chart::draw_chart` draws.
//!
//! # What is read
//!
//! * `chart:class` of the chart and of each series (`bar`, `line`, `area`, `circle` = pie, `ring`,
//!   `scatter`, `radar`, `filled-radar`, `bubble`); `stock`, `gantt`, `surface` and anything else
//!   are not drawn ([`parse`] says `None` and the caller draws the object's replacement picture).
//!   Series of different classes make one chart group each (a column + line chart); series
//!   attached to the secondary axis (`chart:attached-axis`) get a group and a second value axis of
//!   their own.
//! * The plot area's style: `chart:vertical` (horizontal bars), `chart:stacked`,
//!   `chart:percentage`, `chart:three-dimensional` (drawn flat), `chart:lines`,
//!   `chart:symbol-type` / `symbol-name` / `symbol-width`, `chart:interpolation`,
//!   `chart:angle-offset`, `chart:treat-empty-cells`, and the data labels
//!   (`chart:data-label-number`, `-text`, `-symbol`, `chart:label-position`); a series' own style
//!   overrides the plot's.
//! * Axes (`chart:axis`, by `chart:dimension` and `chart:name`): the style's `chart:minimum`,
//!   `maximum`, `interval-major`, `interval-minor-divisor`, `logarithmic`, `reverse-direction`,
//!   `display-label`, `tick-marks-*`, `axis-position`, the line (`svg:stroke-color`, `draw:stroke`),
//!   the text properties and the number format (`style:data-style-name`, a `number:*-style` of the
//!   chart's automatic styles, as an Excel-like format code); the axis title; `chart:grid`
//!   (`major` / `minor`); `chart:categories`.
//! * Titles (`chart:title`, `chart:subtitle` as a second line), the legend (`chart:legend`,
//!   `chart:legend-position`), the chart area, the wall (the plot's fill) and their styles'
//!   fills, strokes and text properties.
//! * The data: the `table:table` inside `chart:chart` (the local table), addressed by
//!   `chart:values-cell-range-address`, `chart:label-cell-address`, `chart:domain` and
//!   `chart:categories` (`local-table.$B$2:.$B$5`, rows and columns counted from 1 over the table
//!   as it is written, header rows and columns included). A range that is not valid, or points
//!   outside the table, is empty. **No spreadsheet is ever opened.**
//! * Colours: a series without a colour gets LibreOffice's default palette ([`LO_PALETTE`], by its
//!   position); each point of a pie / ring with no colour of its own the same way.
//!
//! # Not drawn
//!
//! 3-D (drawn flat), `chart:regression-curve`, `chart:error-indicator`, `chart:mean-value`, stock
//! charts, a data table, manual positions and sizes of titles / legend / plot (the layout is the
//! automatic one), series on a third axis, date axes (a date axis is a category axis with the
//! cells' text), gradient and bitmap fills (the fill colour is used), `chart:data-label-series`.
//!
//! # Budgets
//!
//! The part is read into an element tree of at most [`MAX_CHART_NODES`] nodes
//! ([`MAX_CHART_TEXT`] bytes of text); the local table keeps at most [`MAX_TABLE_ROWS`] rows x
//! [`MAX_TABLE_COLS`] columns ([`MAX_TABLE_CELLS`] cells, repeat counts included); a range is cut at
//! [`ch::MAX_POINTS`] points, the series at [`ch::MAX_SERIES`]. Anything dropped sets
//! [`Parsed::truncated`].

use std::collections::HashMap;

use crate::preview::office::slide_draw as sd;
use sd::chart as ch;
use sd::{Dash, Fill, Rgba};

use super::styles::{text_of, StyleBook, View};
use super::units::{color, pct};
use super::*;

/// Most nodes of a chart part's element tree.
pub(super) const MAX_CHART_NODES: usize = 200_000;
/// Most bytes of text of a chart part's element tree.
pub(super) const MAX_CHART_TEXT: usize = 4 * 1024 * 1024;
/// Most rows of the local table that are kept.
pub(super) const MAX_TABLE_ROWS: usize = 20_000;
/// Most columns of the local table that are kept.
pub(super) const MAX_TABLE_COLS: usize = 256;
/// Most cells (repeat counts included) of the local table that are kept.
pub(super) const MAX_TABLE_CELLS: usize = 200_000;
/// Most values a chart makes of the local table over all its series (cells read as numbers or
/// labels, and the category labels each series holds a copy of). One series is at most
/// [`ch::MAX_POINTS`]; without this a chart of [`ch::MAX_SERIES`] series of that many points,
/// each with its own copy of the labels, is gigabytes. The table itself has at most
/// [`MAX_TABLE_CELLS`] cells, so a chart that plots each cell once is never cut.
pub(super) const MAX_CHART_POINTS: usize = 200_000;
/// Deepest nesting of row / column groups of the local table followed.
const MAX_TABLE_NEST: usize = 4;
/// LibreOffice's default series colours, in order (`004586 ff420e ffd320 579d1c 7e0021 83caff
/// 314004 aecf00 4b1f6f ff950e c5000b 0084d1`).
pub(super) const LO_PALETTE: [Rgba; 12] = [
    Rgba::rgb(0x00, 0x45, 0x86),
    Rgba::rgb(0xff, 0x42, 0x0e),
    Rgba::rgb(0xff, 0xd3, 0x20),
    Rgba::rgb(0x57, 0x9d, 0x1c),
    Rgba::rgb(0x7e, 0x00, 0x21),
    Rgba::rgb(0x83, 0xca, 0xff),
    Rgba::rgb(0x31, 0x40, 0x04),
    Rgba::rgb(0xae, 0xcf, 0x00),
    Rgba::rgb(0x4b, 0x1f, 0x6f),
    Rgba::rgb(0xff, 0x95, 0x0e),
    Rgba::rgb(0xc5, 0x00, 0x0b),
    Rgba::rgb(0x00, 0x84, 0xd1),
];
/// The title size LibreOffice gives a chart title that states none (pt).
const TITLE_PT: f64 = 13.0;
/// The size of the text of a chart that states none (pt).
const TEXT_PT: f64 = 10.0;
/// LibreOffice's default gap between the bars of two categories (percent of a bar).
const DEFAULT_GAP: f64 = 100.0;
/// The axis ids of the model: the category / x axis and the two value axes.
const AX_X: i64 = 1;
const AX_Y: i64 = 2;
const AX_Y2: i64 = 3;

/// A chart read from a part.
pub(super) struct Parsed {
    pub model: ch::ChartModel,
    pub truncated: bool,
}

/// The element tree of a part: its root element (`office:document-content` /
/// `office:document-styles`); `None` when the XML cannot be read or is over the budget.
pub(super) fn read_tree(bytes: &[u8]) -> Option<Node> {
    let mut rd = XmlReader::new(bytes);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        let (e, empty) = match rd.read_event_into(&mut buf).ok()? {
            Event::Start(e) => (e.into_owned(), false),
            Event::Empty(e) => (e.into_owned(), true),
            Event::Eof => return None,
            _ => continue,
        };
        let mut budget = Budget::odf(MAX_CHART_NODES, MAX_CHART_TEXT);
        return match read_element(&mut rd, &e, empty, &mut budget).ok()? {
            Tree::Ok(n) => Some(n),
            Tree::TooBig => None,
        };
    }
}

// ---------------------------------------------------------------------------------------------
// the local table and its addresses
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Default)]
struct Cell {
    num: Option<f64>,
    text: String,
}

#[derive(Default)]
struct Grid {
    rows: Vec<Vec<Cell>>,
    truncated: bool,
}

fn repeat(n: &Node, key: &str) -> usize {
    n.attr(key)
        .and_then(|v| v.trim().parse::<usize>().ok())
        .map_or(1, |v| v.clamp(1, 1_000_000))
}

fn collect_rows<'n>(n: &'n Node, depth: usize, out: &mut Vec<&'n Node>) {
    for k in n.nodes() {
        match k.name.as_str() {
            "table-row" => out.push(k),
            "table-rows" | "table-header-rows" | "table-row-group" if depth < MAX_TABLE_NEST => {
                collect_rows(k, depth + 1, out);
            }
            _ => {}
        }
    }
}

fn read_grid(table: &Node) -> Grid {
    let mut g = Grid::default();
    let mut rows = Vec::new();
    collect_rows(table, 0, &mut rows);
    let mut cells = 0usize;
    'rows: for r in rows {
        let cols: Vec<Cell> = {
            let mut cols: Vec<Cell> = Vec::new();
            for c in r.nodes() {
                if !matches!(c.name.as_str(), "table-cell" | "covered-table-cell") {
                    continue;
                }
                let num = match c.attr("value-type").map(str::trim) {
                    Some("float" | "percentage" | "currency") => {
                        c.attr("value").and_then(|v| v.trim().parse::<f64>().ok())
                    }
                    _ => None,
                }
                .filter(|v| v.is_finite());
                let mut text = String::new();
                for p in c.nodes().filter(|k| k.name == "p") {
                    if !text.is_empty() {
                        text.push('\n');
                    }
                    text_of(p, &mut text, 0);
                }
                let one = Cell { num, text };
                for _ in 0..repeat(c, "number-columns-repeated") {
                    if cols.len() >= MAX_TABLE_COLS {
                        g.truncated = true;
                        break;
                    }
                    cols.push(one.clone());
                }
            }
            cols
        };
        for _ in 0..repeat(r, "number-rows-repeated") {
            if g.rows.len() >= MAX_TABLE_ROWS || cells + cols.len() > MAX_TABLE_CELLS {
                g.truncated = true;
                break 'rows;
            }
            cells += cols.len();
            g.rows.push(cols.clone());
        }
    }
    g
}

/// A cell range, zero-based and inclusive: (column 0, row 0, column 1, row 1).
type Rng = (usize, usize, usize, usize);

/// `$B$2` (after the table name and a dot) as (column, row), zero-based.
fn cell_ref(t: &str) -> Option<(usize, usize)> {
    let t = t.rsplit('.').next()?.trim_start_matches('$');
    let split = t.find(|c: char| !c.is_ascii_alphabetic())?;
    let (letters, rest) = t.split_at(split);
    let digits = rest.trim_start_matches('$');
    if letters.is_empty() || letters.len() > 3 || digits.is_empty() || digits.len() > 8 {
        return None;
    }
    let mut col = 0usize;
    for b in letters.bytes() {
        col = col * 26 + usize::from(b.to_ascii_uppercase() - b'A' + 1);
    }
    let row: usize = digits.parse().ok()?;
    (col >= 1 && row >= 1).then_some((col - 1, row - 1))
}

/// The ranges an address attribute names (`local-table.$B$2:.$B$5`; several separated by white
/// space); an unreadable one is left out.
fn ranges(addr: &str) -> Vec<Rng> {
    addr.split_whitespace()
        .take(64)
        .filter_map(|part| {
            let (a, b) = part.split_once(':').unwrap_or((part, part));
            let (c0, r0) = cell_ref(a)?;
            let (c1, r1) = cell_ref(b)?;
            Some((c0.min(c1), r0.min(r1), c0.max(c1), r0.max(r1)))
        })
        .collect()
}

/// A slot outside the table.
static EMPTY_CELL: Cell = Cell {
    num: None,
    text: String::new(),
};

impl Grid {
    /// `f` of the cells of the ranges, in reading order (a single row left to right, otherwise
    /// down each column); a slot outside the table is an empty cell. At most `limit` values
    /// (and [`ch::MAX_POINTS`]) are made, and no more is looked at: a range of a million slots
    /// costs `limit` steps. The flag says the ranges held more than that.
    fn map_cells<T>(
        &self,
        addr: Option<&str>,
        limit: usize,
        mut f: impl FnMut(&Cell) -> T,
    ) -> (Vec<T>, bool) {
        let limit = limit.min(ch::MAX_POINTS);
        let ranges = addr.map(ranges).unwrap_or_default();
        // The exact size of the result: the vector never holds spare capacity.
        let wanted = ranges.iter().fold(0usize, |n, &(c0, r0, c1, r1)| {
            let cols = c1 - c0 + 1;
            let cols = if r0 == r1 {
                cols
            } else {
                cols.min(MAX_TABLE_COLS + 1)
            };
            n.saturating_add(cols.saturating_mul(r1 - r0 + 1))
        });
        let mut out = Vec::with_capacity(wanted.min(limit));
        let mut cut = false;
        'ranges: for (c0, r0, c1, r1) in ranges {
            let mut push = |c: usize, r: usize, out: &mut Vec<T>| {
                if out.len() >= limit {
                    return false;
                }
                let cell = self
                    .rows
                    .get(r)
                    .and_then(|row| row.get(c))
                    .unwrap_or(&EMPTY_CELL);
                out.push(f(cell));
                true
            };
            if r0 == r1 {
                for c in c0..=c1 {
                    if !push(c, r0, &mut out) {
                        cut = true;
                        break 'ranges;
                    }
                }
            } else {
                // (Columns past the table's width are all empty: only that many are walked.)
                if c1 - c0 > MAX_TABLE_COLS {
                    cut = true;
                }
                for c in c0..=c1.min(c0 + MAX_TABLE_COLS) {
                    for r in r0..=r1 {
                        if !push(c, r, &mut out) {
                            cut = true;
                            break 'ranges;
                        }
                    }
                }
            }
        }
        (out, cut)
    }
}

// ---------------------------------------------------------------------------------------------
// number formats
// ---------------------------------------------------------------------------------------------

fn quoted(s: &str) -> String {
    let s: String = s.chars().filter(|c| *c != '"').collect();
    if s.is_empty() {
        String::new()
    } else {
        format!("\"{s}\"")
    }
}

/// The number part of a `number:*-style` as a format code (`#,##0.00`).
fn number_core(n: &Node) -> String {
    if let Some(sci) = n.child("scientific-number") {
        let d = sci
            .attr("decimal-places")
            .and_then(|v| v.trim().parse::<usize>().ok())
            .unwrap_or(2)
            .min(15);
        return format!("0{}{}E+00", if d > 0 { "." } else { "" }, "0".repeat(d));
    }
    let Some(num) = n.child("number") else {
        return "General".to_string();
    };
    let d = num
        .attr("decimal-places")
        .and_then(|v| v.trim().parse::<usize>().ok())
        .unwrap_or(0)
        .min(15);
    let grouping = num.attr("grouping").map(str::trim) == Some("true");
    let int = if grouping { "#,##0" } else { "0" };
    format!("{int}{}{}", if d > 0 { "." } else { "" }, "0".repeat(d))
}

/// A `number:number-style` / `percentage-style` / `currency-style` / `date-style` /
/// `time-style` as a format code.
fn format_code(n: &Node) -> Option<String> {
    let mut before = String::new();
    let mut after = String::new();
    let mut seen_number = false;
    let mut has_percent = false;
    let mut date = String::new();
    for k in n.nodes() {
        match k.name.as_str() {
            "text" => {
                let mut t = String::new();
                text_of(k, &mut t, 0);
                let t = if matches!(n.name.as_str(), "date-style" | "time-style") {
                    date.push_str(&t);
                    continue;
                } else if n.name == "percentage-style" && t.trim() == "%" {
                    // (The percent sign of a percentage style is a text child: it is the
                    // format's own sign, which also scales the number.)
                    has_percent = true;
                    "%".to_string()
                } else {
                    quoted(&t)
                };
                if seen_number {
                    after.push_str(&t);
                } else {
                    before.push_str(&t);
                }
            }
            "currency-symbol" => {
                let mut t = String::new();
                text_of(k, &mut t, 0);
                let t = quoted(&t);
                if seen_number {
                    after.push_str(&t);
                } else {
                    before.push_str(&t);
                }
            }
            "number" | "scientific-number" => seen_number = true,
            "year" => date.push_str(if k.attr("style") == Some("long") {
                "yyyy"
            } else {
                "yy"
            }),
            "month" => date.push_str(if k.attr("textual") == Some("true") {
                if k.attr("style") == Some("long") {
                    "mmmm"
                } else {
                    "mmm"
                }
            } else if k.attr("style") == Some("long") {
                "mm"
            } else {
                "m"
            }),
            "day" => date.push_str(if k.attr("style") == Some("long") {
                "dd"
            } else {
                "d"
            }),
            "day-of-week" => date.push_str("ddd"),
            "hours" => date.push_str(if k.attr("style") == Some("long") {
                "hh"
            } else {
                "h"
            }),
            "minutes" => date.push_str(if k.attr("style") == Some("long") {
                "mm"
            } else {
                "m"
            }),
            "seconds" => date.push_str(if k.attr("style") == Some("long") {
                "ss"
            } else {
                "s"
            }),
            _ => {}
        }
    }
    match n.name.as_str() {
        "number-style" => Some(format!("{before}{}{after}", number_core(n))),
        "percentage-style" => Some(format!(
            "{before}{}{}{after}",
            number_core(n),
            if has_percent { "" } else { "%" }
        )),
        "currency-style" => Some(format!("{before}{}{after}", number_core(n))),
        "date-style" | "time-style" if !date.is_empty() => Some(date),
        _ => None,
    }
}

// ---------------------------------------------------------------------------------------------
// styles
// ---------------------------------------------------------------------------------------------

fn pts(v: &str) -> Option<f64> {
    emu(v).map(|e| e / sd::EMU_PER_PT)
}

fn text_style(v: &View) -> ch::TextStyle {
    ch::TextStyle {
        size_pt: v.t("font-size").and_then(pts).filter(|s| *s > 0.0),
        bold: v.t("font-weight").map(|w| {
            let w = w.trim();
            w == "bold" || w.parse::<u32>().is_ok_and(|n| n >= 600)
        }),
        italic: v
            .t("font-style")
            .map(|s| matches!(s.trim(), "italic" | "oblique")),
        color: v.t("color").and_then(color),
        font: v
            .t("font-name")
            .or_else(|| v.t("font-family"))
            .map(super::styles::clean_family)
            .filter(|s| !s.is_empty()),
        rot_deg: None,
    }
}

fn fill_of(v: &View) -> Option<Fill> {
    let alpha = v
        .g("opacity")
        .and_then(pct)
        .filter(|a| (0.0..=1.0).contains(a))
        .unwrap_or(1.0);
    match v.g("fill").map(str::trim) {
        Some("none") => None,
        _ => v
            .g("fill-color")
            .and_then(color)
            .map(|c| Fill::Solid(c.with_alpha(alpha))),
    }
}

fn stroke_of(v: &View) -> Option<ch::Stroke> {
    let width = v
        .g("stroke-width")
        .and_then(emu)
        .filter(|w| *w > 0.0)
        .map(|w| w.min(1.0e8));
    let col = v.g("stroke-color").and_then(color);
    match v.g("stroke").map(str::trim) {
        Some("none") => Some(ch::Stroke {
            none: true,
            ..ch::Stroke::default()
        }),
        Some(kind) => Some(ch::Stroke {
            none: false,
            color: col,
            width,
            dash: if kind == "dash" {
                Dash::Dash
            } else {
                Dash::Solid
            },
        }),
        None if col.is_some() || width.is_some() => Some(ch::Stroke {
            none: false,
            color: col,
            width,
            dash: Dash::Solid,
        }),
        None => None,
    }
}

/// `chart:*` (and `style:*`) property of a style view.
fn cp<'a>(v: &View<'a>, key: &str) -> Option<&'a str> {
    v.x("chart-properties", key)
}

fn truthy(v: Option<&str>) -> Option<bool> {
    v.map(|s| matches!(s.trim(), "true" | "1"))
}

fn num_prop(v: &View, key: &str) -> Option<f64> {
    cp(v, key)
        .and_then(|s| s.trim().parse::<f64>().ok())
        .filter(|n| n.is_finite())
}

/// The marker symbol of a name (`named-symbol`).
pub(super) fn symbol_named(name: &str) -> ch::MarkerSymbol {
    use ch::MarkerSymbol as M;
    match name.trim() {
        "square" => M::Square,
        "diamond" => M::Diamond,
        "arrow-up" => M::Triangle,
        "arrow-down" => M::TriangleDown,
        "arrow-left" => M::TriangleLeft,
        "arrow-right" => M::TriangleRight,
        "circle" => M::Circle,
        "star" | "asterisk" => M::Star,
        "x" => M::X,
        "bowtie" => M::Bowtie,
        "sandglass" => M::Sandglass,
        "plus" => M::Plus,
        "horizontal-bar" => M::Dash,
        "vertical-bar" => M::VBar,
        _ => M::Diamond,
    }
}

/// The symbol LibreOffice picks for the `i`-th series of an automatic symbol set.
fn symbol_auto(i: usize) -> ch::MarkerSymbol {
    const CYCLE: [&str; 15] = [
        "square",
        "diamond",
        "arrow-down",
        "arrow-up",
        "arrow-right",
        "arrow-left",
        "bowtie",
        "sandglass",
        "circle",
        "star",
        "x",
        "plus",
        "asterisk",
        "horizontal-bar",
        "vertical-bar",
    ];
    symbol_named(CYCLE[i % CYCLE.len()])
}

// ---------------------------------------------------------------------------------------------
// the chart
// ---------------------------------------------------------------------------------------------

/// The class of a chart or a series, without its prefix.
fn class_of(n: &Node) -> Option<&str> {
    n.attr("class")
        .map(|c| c.trim().trim_start_matches("chart:"))
}

fn kind_of(class: &str) -> Option<ch::GroupKind> {
    Some(match class {
        "bar" => ch::GroupKind::Bar,
        "line" => ch::GroupKind::Line,
        "area" => ch::GroupKind::Area,
        "circle" => ch::GroupKind::Pie,
        "ring" => ch::GroupKind::Doughnut,
        "scatter" => ch::GroupKind::Scatter,
        "radar" | "filled-radar" => ch::GroupKind::Radar,
        "bubble" => ch::GroupKind::Bubble,
        _ => return None,
    })
}

/// What the builder shares.
struct Cx<'a> {
    book: &'a StyleBook,
    formats: HashMap<String, String>,
    grid: Grid,
    truncated: bool,
    /// How many values (cells made into numbers or labels, and the category labels each series
    /// holds a copy of) the chart may still make: [`MAX_CHART_POINTS`] over all its series.
    points_left: usize,
}

impl<'a> Cx<'a> {
    /// `f` of the cells of `addr`, within what is left of the chart's points.
    fn cells_with<T>(&mut self, addr: Option<&str>, f: impl FnMut(&Cell) -> T) -> Vec<T> {
        let (out, cut) = self.grid.map_cells(addr, self.points_left, f);
        self.points_left -= out.len();
        if cut {
            self.truncated = true;
        }
        out
    }

    /// The name of a series: the text of the first cell of its label address (a longer range is
    /// not an error, the rest is just not used).
    fn series_label(&mut self, addr: Option<&str>) -> Option<String> {
        let limit = self.points_left.min(1);
        let (first, _) = self.grid.map_cells(addr, limit, |c| {
            let mut t = c.text.clone();
            ch::cut_chars(&mut t, ch::MAX_LABEL_CHARS);
            t
        });
        self.points_left -= first.len();
        first.into_iter().next().filter(|t| !t.trim().is_empty())
    }

    /// The category labels and the numbers the same cells hold, one value each against the budget.
    fn categories(&mut self, addr: Option<&str>) -> (Vec<String>, Vec<Option<f64>>) {
        let both = self.cells_with(addr, |c| {
            let mut t = c.text.clone();
            ch::cut_chars(&mut t, ch::MAX_LABEL_CHARS);
            (t, c.num)
        });
        both.into_iter().unzip()
    }

    fn numbers(&mut self, addr: Option<&str>) -> Vec<Option<f64>> {
        self.cells_with(addr, |c| c.num)
    }

    /// The first `cats.len()` category labels a series holds a copy of, within the chart's points.
    fn copy_cats(
        &mut self,
        cats: &[String],
        nums: &[Option<f64>],
    ) -> (Vec<String>, Vec<Option<f64>>) {
        let n = cats.len().min(self.points_left);
        if n < cats.len() {
            self.truncated = true;
        }
        self.points_left -= n;
        (cats[..n].to_vec(), nums[..n.min(nums.len())].to_vec())
    }

    fn view(&self, n: &Node) -> View<'a> {
        let mut v = View::default();
        if let Some(def) = self.book.default_style("chart") {
            v.push(def);
        }
        if let Some(s) = qattr(n, "chart:style-name") {
            v.push_chain(self.book, "chart", s);
        }
        v
    }

    /// The format code the style's `style:data-style-name` names.
    fn format_of(&self, v: &View) -> Option<String> {
        v.data_style()
            .and_then(|n| self.formats.get(n.trim()).cloned())
    }

    /// The text of a `chart:title` / `chart:subtitle`.
    fn title_text(&self, t: &Node) -> String {
        let mut out = String::new();
        for p in t.nodes().filter(|k| k.name == "p") {
            if !out.is_empty() {
                out.push('\n');
            }
            text_of(p, &mut out, 0);
        }
        let mut out = out.trim().to_string();
        ch::cut_chars(&mut out, ch::MAX_LABEL_CHARS);
        out
    }

    fn title(&self, t: &Node) -> Option<ch::ChartText> {
        let text = self.title_text(t);
        if text.is_empty() {
            return None;
        }
        let v = self.view(t);
        let mut style = text_style(&v);
        // The first span of the title's text may have a style of its own (LibreOffice writes
        // the size of an axis title that way).
        let span = t
            .nodes()
            .find(|k| k.name == "p")
            .and_then(|p| p.nodes().find(|k| k.name == "span"))
            .and_then(|sp| qattr(sp, "text:style-name"));
        if let Some(name) = span {
            let mut sv = View::default();
            sv.push_chain(self.book, "text", name);
            style = text_style(&sv).over(&style);
        }
        style.size_pt.get_or_insert(TITLE_PT);
        // (LibreOffice's titles are not bold; the drawing's own default for a title is.)
        style.bold.get_or_insert(false);
        style.rot_deg = cp(&v, "rotation-angle")
            .and_then(|a| a.trim().parse::<f64>().ok())
            .filter(|a| a.is_finite())
            .map(|a| -a);
        Some(ch::ChartText {
            text,
            style,
            layout: None,
            overlay: false,
        })
    }
}

/// The chart of the part's root elements; `None` when it is not a chart that can be drawn.
pub(super) fn parse(content: &Node, styles: Option<&Node>) -> Option<Parsed> {
    let chart = content.child("body")?.child("chart")?.child("chart")?;
    let class = class_of(chart)?;
    kind_of(class)?;

    // Styles: the part's styles.xml first, then its content.xml.
    let mut book = StyleBook::default();
    let mut formats: HashMap<String, String> = HashMap::new();
    let mut containers: Vec<&Node> = Vec::new();
    if let Some(s) = styles {
        containers.extend(
            s.nodes()
                .filter(|n| matches!(n.name.as_str(), "styles" | "automatic-styles")),
        );
    }
    containers.extend(content.nodes().filter(|n| n.name == "automatic-styles"));
    for c in &containers {
        book.add(c);
        for n in c.nodes() {
            if matches!(
                n.name.as_str(),
                "number-style"
                    | "percentage-style"
                    | "currency-style"
                    | "date-style"
                    | "time-style"
            ) {
                if let (Some(name), Some(code)) = (n.attr("name"), format_code(n)) {
                    if formats.len() < 512 {
                        formats.insert(name.to_string(), code);
                    }
                }
            }
        }
    }
    let grid = chart.child("table").map(read_grid).unwrap_or_default();
    let mut cx = Cx {
        book: &book,
        formats,
        truncated: grid.truncated,
        grid,
        points_left: MAX_CHART_POINTS,
    };
    let model = build(&mut cx, chart)?;
    let truncated = cx.truncated || book.truncated;
    Some(Parsed { model, truncated })
}

fn build(cx: &mut Cx<'_>, chart: &Node) -> Option<ch::ChartModel> {
    let chart_class = class_of(chart)?.to_string();
    let chart_view = cx.view(chart);
    let mut m = ch::ChartModel {
        palette: LO_PALETTE[..6].to_vec(),
        libre_scaling: true,
        text: ch::TextStyle {
            size_pt: Some(TEXT_PT),
            ..text_style(&chart_view)
        },
        ..ch::ChartModel::default()
    };
    m.chart_fill = fill_of(&chart_view);
    m.chart_line = stroke_of(&chart_view);

    // Titles.
    let title = chart.child("title").and_then(|t| cx.title(t));
    let subtitle = chart.child("subtitle").and_then(|t| cx.title(t));
    m.title = match (title, subtitle) {
        (Some(mut t), Some(s)) => {
            t.text = format!("{}\n{}", t.text, s.text);
            Some(t)
        }
        (Some(t), None) => Some(t),
        (None, Some(s)) => Some(s),
        (None, None) => None,
    };

    // The legend.
    if let Some(l) = chart.child("legend") {
        let v = cx.view(l);
        let pos = match l.attr("legend-position").map(str::trim) {
            Some("start") => ch::LegendPos::Left,
            Some("top" | "top-start") => ch::LegendPos::Top,
            Some("top-end") => ch::LegendPos::TopRight,
            Some("bottom" | "bottom-start" | "bottom-end") => ch::LegendPos::Bottom,
            Some("start-top" | "start-bottom") => ch::LegendPos::Left,
            _ => ch::LegendPos::Right,
        };
        m.legend = Some(ch::Legend {
            pos,
            overlay: false,
            layout: None,
            style: text_style(&v),
            fill: fill_of(&v),
            // (A legend with no stated outline has LibreOffice's thin grey one.)
            line: stroke_of(&v).or(Some(ch::Stroke {
                none: false,
                color: Some(Rgba::rgb(0x80, 0x80, 0x80)),
                width: Some(9525.0),
                dash: Dash::Solid,
            })),
            deleted: Vec::new(),
        });
    }

    let plot = chart.child("plot-area")?;
    let plot_view = cx.view(plot);
    m.three_d = truthy(cp(&plot_view, "three-dimensional")).unwrap_or(false);
    m.disp_blanks_as = match cp(&plot_view, "treat-empty-cells").map(str::trim) {
        Some("use-zero") => ch::DispBlanks::Zero,
        Some("ignore" | "continue") => ch::DispBlanks::Span,
        _ => ch::DispBlanks::Gap,
    };
    if let Some(w) = plot.child("wall") {
        let v = cx.view(w);
        m.plot_fill = fill_of(&v);
        m.plot_line = stroke_of(&v);
    }

    // Axes by dimension.
    let axes: Vec<&Node> = plot.nodes().filter(|n| n.name == "axis").collect();
    let axis_named = |dim: &str, secondary: bool| {
        axes.iter().copied().find(|a| {
            a.attr("dimension").map(str::trim) == Some(dim)
                && a.attr("name").is_some_and(|n| n.contains("secondary")) == secondary
        })
    };
    let x_axis = axis_named("x", false);
    let y_axis = axis_named("y", false);
    let y2_axis = axis_named("y", true);

    // Categories (the x axis' `chart:categories`).
    let cat_addr = x_axis
        .and_then(|a| a.child("categories"))
        .and_then(|c| qattr(c, "table:cell-range-address"));
    let (cats, cat_nums) = cx.categories(cat_addr);

    // Series -> groups (one per class and axis, in order of first appearance).
    let vertical = truthy(cp(&plot_view, "vertical")).unwrap_or(false);
    let stacked = truthy(cp(&plot_view, "stacked")).unwrap_or(false);
    let percentage = truthy(cp(&plot_view, "percentage")).unwrap_or(false);
    let mut keys: Vec<(String, bool)> = Vec::new();
    let mut groups: Vec<ch::ChartGroup> = Vec::new();
    let mut index = 0usize;
    let mut any_secondary = false;
    for s in plot.nodes().filter(|n| n.name == "series") {
        if index >= ch::MAX_SERIES {
            cx.truncated = true;
            break;
        }
        // (A ring chart's series say `circle`; the chart's own class is the one that counts for
        // the pie family. For the others a series' class is its own: a column + line chart.)
        let class = if matches!(chart_class.as_str(), "ring" | "circle") {
            chart_class.clone()
        } else {
            class_of(s).unwrap_or(&chart_class).to_string()
        };
        let Some(kind) = kind_of(&class) else {
            continue;
        };
        let secondary = s
            .attr("attached-axis")
            .is_some_and(|a| a.contains("secondary"));
        any_secondary |= secondary;
        let gi = match keys.iter().position(|k| k.0 == class && k.1 == secondary) {
            Some(i) => i,
            None => {
                keys.push((class.clone(), secondary));
                let mut g = ch::ChartGroup {
                    kind,
                    bar_dir: if vertical && kind == ch::GroupKind::Bar {
                        ch::BarDir::Bar
                    } else {
                        ch::BarDir::Col
                    },
                    three_d: m.three_d,
                    ..ch::ChartGroup::default()
                };
                g.grouping = if percentage {
                    ch::Grouping::PercentStacked
                } else if stacked {
                    ch::Grouping::Stacked
                } else if kind == ch::GroupKind::Bar {
                    ch::Grouping::Clustered
                } else {
                    ch::Grouping::Standard
                };
                g.vary_colors = matches!(kind, ch::GroupKind::Pie | ch::GroupKind::Doughnut);
                let ay = if secondary { y2_axis } else { y_axis };
                let gap = ay
                    .map(|a| cx.view(a))
                    .and_then(|v| num_prop(&v, "gap-width"))
                    .or_else(|| num_prop(&plot_view, "gap-width"));
                g.gap_width = gap.unwrap_or(DEFAULT_GAP);
                let overlap = ay
                    .map(|a| cx.view(a))
                    .and_then(|v| num_prop(&v, "overlap"))
                    .or_else(|| num_prop(&plot_view, "overlap"));
                g.overlap = overlap.unwrap_or(0.0);
                g.axis_ids = match kind {
                    ch::GroupKind::Pie | ch::GroupKind::Doughnut => Vec::new(),
                    _ => vec![AX_X, if secondary { AX_Y2 } else { AX_Y }],
                };
                // LibreOffice's start angle counts counter-clockwise from three o'clock (90 =
                // twelve o'clock, its default); the model's is clockwise from twelve.
                let ao = num_prop(&plot_view, "angle-offset").unwrap_or(90.0);
                g.first_slice_ang = (90.0 - ao).rem_euclid(360.0);
                g.hole_size = 50.0;
                g.counter_clockwise = true;
                groups.push(g);
                groups.len() - 1
            }
        };
        let series = series_of(cx, s, kind, &class, index, &plot_view, &cats, &cat_nums);
        let g = &mut groups[gi];
        // Group-level flags that follow the series' style.
        match kind {
            ch::GroupKind::Line | ch::GroupKind::Radar => {
                g.markers = series
                    .marker
                    .as_ref()
                    .is_some_and(|k| k.symbol != ch::MarkerSymbol::None);
            }
            ch::GroupKind::Scatter | ch::GroupKind::Bubble => {
                let lines = scatter_lines(&cx.view(s), &plot_view, kind);
                let markers = series
                    .marker
                    .as_ref()
                    .is_none_or(|k| k.symbol != ch::MarkerSymbol::None);
                let smooth = series.smooth == Some(true);
                g.scatter_style = match (lines, markers, smooth) {
                    (true, true, false) => ch::ScatterStyle::LineMarker,
                    (true, true, true) => ch::ScatterStyle::SmoothMarker,
                    (true, false, false) => ch::ScatterStyle::Line,
                    (true, false, true) => ch::ScatterStyle::Smooth,
                    (false, true, _) => ch::ScatterStyle::Marker,
                    (false, false, _) => ch::ScatterStyle::None,
                };
            }
            _ => {}
        }
        if class == "filled-radar" {
            g.radar_style = ch::RadarStyle::Filled;
        } else if kind == ch::GroupKind::Radar {
            g.radar_style = if g.markers {
                ch::RadarStyle::Marker
            } else {
                ch::RadarStyle::Standard
            };
        }
        g.series.push(series);
        index += 1;
    }
    if groups.is_empty() {
        return None;
    }
    // LibreOffice paints overlapping areas back to front in reverse series order (the first series
    // is in front); the model paints in series order.
    for g in &mut groups {
        if g.kind == ch::GroupKind::Area && g.grouping == ch::Grouping::Standard {
            g.series.reverse();
        }
    }
    m.groups = groups;

    // Axes of the model.
    let cartesian = m.groups.iter().any(|g| !g.axis_ids.is_empty());
    if cartesian {
        let xy = m
            .groups
            .iter()
            .any(|g| matches!(g.kind, ch::GroupKind::Scatter | ch::GroupKind::Bubble));
        let area_only = m
            .groups
            .iter()
            .all(|g| matches!(g.kind, ch::GroupKind::Area));
        let horizontal = m
            .groups
            .iter()
            .any(|g| g.kind == ch::GroupKind::Bar && g.bar_dir == ch::BarDir::Bar);
        let percent = m
            .groups
            .iter()
            .any(|g| g.grouping == ch::Grouping::PercentStacked);
        let (cat_pos, val_pos, val2_pos) = if horizontal {
            (ch::AxisPos::Left, ch::AxisPos::Bottom, ch::AxisPos::Top)
        } else {
            (ch::AxisPos::Bottom, ch::AxisPos::Left, ch::AxisPos::Right)
        };
        let mut ax = axis_of(
            cx,
            x_axis,
            AX_X,
            if xy {
                ch::AxisKind::Val
            } else {
                ch::AxisKind::Cat
            },
            cat_pos,
            AX_Y,
        );
        ax.between = !(xy || area_only);
        m.axes.push(ax);
        let mut ay = axis_of(cx, y_axis, AX_Y, ch::AxisKind::Val, val_pos, AX_X);
        if percent && ay.num_fmt.is_none() {
            ay.num_fmt = Some(("0%".to_string(), false));
        }
        ay.between = !(xy || area_only);
        m.axes.push(ay);
        if any_secondary {
            let mut a2 = axis_of(cx, y2_axis, AX_Y2, ch::AxisKind::Val, val2_pos, AX_X);
            a2.crosses = ch::Crosses::Max;
            m.axes.push(a2);
        }
    }
    Some(m)
}

/// Whether a scatter / bubble series draws its line: the style's `chart:lines` (the series',
/// else the plot's); a scatter chart without a statement has them, as in LibreOffice.
fn scatter_lines(series: &View, plot: &View, kind: ch::GroupKind) -> bool {
    if kind == ch::GroupKind::Bubble {
        return false;
    }
    truthy(cp(series, "lines"))
        .or_else(|| truthy(cp(plot, "lines")))
        .unwrap_or(true)
}

#[allow(clippy::too_many_arguments)]
fn series_of(
    cx: &mut Cx<'_>,
    s: &Node,
    kind: ch::GroupKind,
    class: &str,
    index: usize,
    plot: &View,
    cats: &[String],
    cat_nums: &[Option<f64>],
) -> ch::Series {
    let v = cx.view(s);
    let mut out = ch::Series::default();
    let addr = |key: &str| qattr(s, &format!("chart:{key}"));
    let name = cx.series_label(addr("label-cell-address"));
    out.name = name;
    let domains: Vec<Option<&str>> = s
        .nodes()
        .filter(|n| n.name == "domain")
        .take(2)
        .map(|d| qattr(d, "table:cell-range-address"))
        .collect();
    let values_addr = addr("values-cell-range-address");
    match kind {
        ch::GroupKind::Scatter => {
            out.x_values = cx.numbers(domains.first().copied().flatten());
            out.values = cx.numbers(values_addr);
        }
        ch::GroupKind::Bubble => {
            // Values are the bubble sizes, the first domain the x values, the second the y values.
            out.sizes = cx.numbers(values_addr);
            out.x_values = cx.numbers(domains.first().copied().flatten());
            out.values = cx.numbers(domains.get(1).copied().flatten());
        }
        _ => {
            out.values = cx.numbers(values_addr);
            (out.cats, out.cat_nums) = cx.copy_cats(cats, cat_nums);
        }
    }
    out.format_code = cx.format_of(&v);

    let own = LO_PALETTE[index % LO_PALETTE.len()];
    let fill_color = v.g("fill-color").and_then(color);
    let stroke = stroke_of(&v);
    let stroke_color = v.g("stroke-color").and_then(color);
    let fills = !matches!(v.g("fill").map(str::trim), Some("none"));
    let alpha = v
        .g("opacity")
        .and_then(pct)
        .filter(|a| (0.0..=1.0).contains(a))
        .unwrap_or(1.0);

    match kind {
        ch::GroupKind::Line | ch::GroupKind::Scatter | ch::GroupKind::Radar => {
            let main = stroke_color.or(fill_color).unwrap_or(own);
            let lines = match kind {
                ch::GroupKind::Scatter => scatter_lines(&v, plot, kind),
                _ => truthy(cp(&v, "lines"))
                    .or_else(|| truthy(cp(plot, "lines")))
                    .unwrap_or(true),
            };
            out.line = Some(if lines && class != "filled-radar" {
                ch::Stroke {
                    none: false,
                    color: Some(main),
                    width: stroke.as_ref().and_then(|s| s.width),
                    dash: stroke.as_ref().map_or(Dash::Solid, |s| s.dash.clone()),
                }
            } else {
                ch::Stroke {
                    none: true,
                    ..ch::Stroke::default()
                }
            });
            if class == "filled-radar" {
                out.fill = Some(Fill::Solid(fill_color.unwrap_or(main).with_alpha(alpha)));
                out.line = Some(ch::Stroke {
                    none: false,
                    color: Some(main),
                    ..ch::Stroke::default()
                });
            }
            // Symbols.
            let sym = cp(&v, "symbol-type").or_else(|| cp(plot, "symbol-type"));
            let symbol = match sym.map(str::trim) {
                Some("automatic") => Some(symbol_auto(index)),
                Some("named-symbol") => Some(symbol_named(
                    cp(&v, "symbol-name")
                        .or_else(|| cp(plot, "symbol-name"))
                        .unwrap_or("diamond"),
                )),
                Some("image") => Some(ch::MarkerSymbol::Circle),
                _ => None,
            };
            out.marker = Some(match symbol {
                Some(symbol) => {
                    let size = cp(&v, "symbol-width")
                        .or_else(|| cp(plot, "symbol-width"))
                        .and_then(pts);
                    ch::Marker {
                        symbol,
                        size_pt: size,
                        fill: Some(Fill::Solid(fill_color.unwrap_or(main))),
                        line: ch::Stroke {
                            none: false,
                            color: Some(stroke_color.unwrap_or(main)),
                            ..ch::Stroke::default()
                        },
                    }
                }
                None => ch::Marker {
                    symbol: ch::MarkerSymbol::None,
                    ..ch::Marker::default()
                },
            });
            let interp = cp(&v, "interpolation").or_else(|| cp(plot, "interpolation"));
            out.smooth = Some(matches!(
                interp.map(str::trim),
                Some("cubic-spline" | "b-spline")
            ));
        }
        _ => {
            out.fill = Some(if fills {
                Fill::Solid(fill_color.unwrap_or(own).with_alpha(alpha))
            } else {
                Fill::None
            });
            out.line = stroke;
            if kind == ch::GroupKind::Bubble {
                out.marker = None;
            }
        }
    }
    if let Some(off) = num_prop(&v, "pie-offset") {
        out.explosion = Some(off.clamp(0.0, 100.0));
    }

    // Data points: a style per point (repeated), the first points' colours for a pie.
    let mut idx = 0usize;
    for dp in s.nodes().filter(|n| n.name == "data-point") {
        let rep = repeat(dp, "repeated");
        let pv = cx.view(dp);
        let has_style = qattr(dp, "chart:style-name").is_some();
        for _ in 0..rep {
            if idx >= ch::MAX_POINTS {
                cx.truncated = true;
                break;
            }
            if has_style {
                let pf = ch::PointFmt {
                    idx,
                    fill: pv
                        .g("fill-color")
                        .and_then(color)
                        .map(|c| Fill::Solid(c.with_alpha(alpha))),
                    line: stroke_of(&pv),
                    marker: None,
                    explosion: num_prop(&pv, "pie-offset").map(|e| e.clamp(0.0, 100.0)),
                };
                if pf.fill.is_some() || pf.line.is_some() || pf.explosion.is_some() {
                    out.points.push(pf);
                }
            }
            idx += 1;
        }
    }
    if matches!(kind, ch::GroupKind::Pie | ch::GroupKind::Doughnut) {
        // Every slice has a colour: its own, else LibreOffice's palette by its position.
        let n = out.values.len().min(ch::MAX_POINTS);
        for i in 0..n {
            match out.points.iter_mut().find(|p| p.idx == i) {
                Some(p) if p.fill.is_some() => {}
                Some(p) => p.fill = Some(Fill::Solid(LO_PALETTE[i % LO_PALETTE.len()])),
                None => out.points.push(ch::PointFmt {
                    idx: i,
                    fill: Some(Fill::Solid(LO_PALETTE[i % LO_PALETTE.len()])),
                    ..ch::PointFmt::default()
                }),
            }
        }
    }

    out.labels = labels_of(&v, plot, kind, class, cx.format_of(&v));
    out
}

/// The data labels a series' style asks for.
fn labels_of(
    v: &View,
    plot: &View,
    kind: ch::GroupKind,
    _class: &str,
    num_fmt: Option<String>,
) -> Option<ch::DataLabels> {
    let number = cp(v, "data-label-number")
        .or_else(|| cp(plot, "data-label-number"))
        .map_or("none", str::trim);
    let text =
        truthy(cp(v, "data-label-text").or_else(|| cp(plot, "data-label-text"))).unwrap_or(false);
    let symbol = truthy(cp(v, "data-label-symbol").or_else(|| cp(plot, "data-label-symbol")))
        .unwrap_or(false);
    let show_value = matches!(number, "value" | "value-and-percentage");
    let show_percent = matches!(number, "percentage" | "value-and-percentage");
    if !(show_value || show_percent || text) {
        return None;
    }
    let stacked = truthy(cp(plot, "stacked")).unwrap_or(false)
        || truthy(cp(plot, "percentage")).unwrap_or(false);
    let pos = match cp(v, "label-position")
        .or_else(|| cp(plot, "label-position"))
        .map(str::trim)
    {
        Some("outside") => Some(ch::LabelPos::OutsideEnd),
        Some("inside") => Some(ch::LabelPos::InsideEnd),
        Some("center") => Some(ch::LabelPos::Center),
        Some("near-origin") => Some(ch::LabelPos::InsideBase),
        Some("top") => Some(ch::LabelPos::Above),
        Some("bottom") => Some(ch::LabelPos::Below),
        Some("left") => Some(ch::LabelPos::Left),
        Some("right") => Some(ch::LabelPos::Right),
        Some("avoid-overlap") => Some(ch::LabelPos::BestFit),
        _ => Some(match kind {
            ch::GroupKind::Bar if stacked => ch::LabelPos::Center,
            ch::GroupKind::Bar => ch::LabelPos::OutsideEnd,
            ch::GroupKind::Pie | ch::GroupKind::Doughnut => ch::LabelPos::BestFit,
            _ => ch::LabelPos::Above,
        }),
    };
    Some(ch::DataLabels {
        delete: false,
        show_value,
        show_category: text,
        show_series: false,
        show_percent,
        show_legend_key: symbol,
        pos,
        num_fmt: if show_percent && !show_value {
            None
        } else {
            num_fmt
        },
        separator: None,
        style: text_style(v),
        points: Vec::new(),
        text: None,
    })
}

fn axis_of(
    cx: &Cx<'_>,
    node: Option<&Node>,
    id: i64,
    kind: ch::AxisKind,
    pos: ch::AxisPos,
    cross: i64,
) -> ch::Axis {
    let mut ax = ch::Axis {
        id,
        kind,
        cross_ax: cross,
        pos,
        ..ch::Axis::default()
    };
    let Some(n) = node else {
        ax.deleted = true;
        return ax;
    };
    let v = cx.view(n);
    ax.deleted = n.attr("visible").map(str::trim) == Some("false");
    ax.reversed = truthy(cp(&v, "reverse-direction")).unwrap_or(false);
    ax.min = num_prop(&v, "minimum");
    ax.max = num_prop(&v, "maximum");
    if truthy(cp(&v, "logarithmic")).unwrap_or(false) {
        ax.log_base = Some(10.0);
    }
    ax.major_unit = num_prop(&v, "interval-major").filter(|u| *u > 0.0);
    if let (Some(major), Some(div)) = (ax.major_unit, num_prop(&v, "interval-minor-divisor")) {
        if div >= 1.0 {
            ax.minor_unit = Some(major / div);
        }
    }
    let tick = |inner: &str, outer: &str, default_outer: bool| {
        let i = truthy(cp(&v, inner)).unwrap_or(false);
        let o = truthy(cp(&v, outer)).unwrap_or(default_outer);
        match (i, o) {
            (true, true) => ch::TickMark::Cross,
            (true, false) => ch::TickMark::In,
            (false, true) => ch::TickMark::Out,
            (false, false) => ch::TickMark::None,
        }
    };
    ax.major_tick = tick("tick-marks-major-inner", "tick-marks-major-outer", true);
    ax.minor_tick = tick("tick-marks-minor-inner", "tick-marks-minor-outer", false);
    if truthy(cp(&v, "display-label")) == Some(false) {
        ax.tick_label_pos = ch::TickLabelPos::None;
    }
    ax.crosses = match cp(&v, "axis-position").map(str::trim) {
        Some("start") => ch::Crosses::Min,
        Some("end") => ch::Crosses::Max,
        Some("0") | None => ch::Crosses::AutoZero,
        Some(x) => x
            .parse::<f64>()
            .ok()
            .filter(|f| f.is_finite())
            .map_or(ch::Crosses::AutoZero, ch::Crosses::At),
    };
    if let Some(code) = cx.format_of(&v) {
        ax.num_fmt = Some((
            code,
            truthy(cp(&v, "link-data-style-to-source")).unwrap_or(false),
        ));
    }
    ax.line = stroke_of(&v);
    ax.text = text_style(&v);
    if let Some(t) = n.child("title") {
        ax.title = cx.title(t);
    }
    for g in n.nodes().filter(|k| k.name == "grid") {
        let gv = cx.view(g);
        let st = stroke_of(&gv).unwrap_or_default();
        match g.attr("class").map(str::trim) {
            Some("minor") => ax.minor_grid = Some(st),
            _ => ax.major_grid = Some(st),
        }
    }
    ax
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid(rows: usize, cols: usize) -> Grid {
        Grid {
            rows: (0..rows)
                .map(|r| {
                    (0..cols)
                        .map(|c| Cell {
                            num: Some((r * 1000 + c) as f64),
                            text: format!("{r}.{c}"),
                        })
                        .collect()
                })
                .collect(),
            truncated: false,
        }
    }

    #[test]
    fn a_column_range_reads_down_and_a_row_range_across() {
        let g = grid(3, 3);
        let (v, cut) = g.map_cells(Some("local-table.$B$1:.$B$3"), 100, |c| c.num);
        assert_eq!(v, vec![Some(1.0), Some(1001.0), Some(2001.0)]);
        assert!(!cut);
        let (v, _) = g.map_cells(Some("local-table.$A$2:.$C$2"), 100, |c| c.num);
        assert_eq!(v, vec![Some(1000.0), Some(1001.0), Some(1002.0)]);
        // Several ranges, in order; the slots outside the table are empty.
        let (v, _) = g.map_cells(Some("local-table.$A$1 local-table.$D$9"), 100, |c| c.num);
        assert_eq!(v, vec![Some(0.0), None]);
    }

    #[test]
    fn a_huge_range_stops_at_the_limit_and_says_so() {
        let g = grid(2, 2);
        for limit in [0usize, 1, 7, ch::MAX_POINTS] {
            let (v, cut) = g.map_cells(Some("local-table.$A$1:.$IV$20000"), limit, |c| c.num);
            assert_eq!(v.len(), limit, "limit {limit}");
            assert!(cut, "limit {limit}");
            // (No spare room: the vector is as long as what it holds.)
            assert_eq!(v.capacity(), v.len(), "limit {limit}");
        }
        // A single row of a million slots too.
        let (v, cut) = g.map_cells(Some("local-table.$A$1:.$ZZZ$1"), 50, |c| c.num);
        assert_eq!(v.len(), 50);
        assert!(cut);
    }

    #[test]
    fn the_limit_is_never_above_the_points_of_a_series() {
        let g = grid(1, 1);
        let (v, cut) = g.map_cells(Some("local-table.$A$1:.$A$30000"), usize::MAX, |c| c.num);
        assert_eq!(v.len(), ch::MAX_POINTS);
        assert!(cut);
    }

    #[test]
    fn a_range_that_just_fits_is_not_cut() {
        let g = grid(1, 1);
        let (v, cut) = g.map_cells(Some("local-table.$A$1:.$A$5"), 5, |c| c.num);
        assert_eq!(v.len(), 5);
        assert!(!cut);
        let (v, cut) = g.map_cells(None, 5, |c| c.num);
        assert!(v.is_empty() && !cut);
        let (v, cut) = g.map_cells(Some("garbage"), 5, |c| c.num);
        assert!(v.is_empty() && !cut);
    }

    #[test]
    fn a_wide_block_is_walked_only_as_wide_as_a_table_can_be() {
        let g = grid(1, 1);
        // 3 rows x 18,000 columns: the columns past the table's width are all empty slots.
        let (v, cut) = g.map_cells(Some("local-table.$A$1:.$ZZ$3"), ch::MAX_POINTS, |c| c.num);
        assert!(cut);
        assert_eq!(v.len(), (MAX_TABLE_COLS + 1) * 3);
    }
}
