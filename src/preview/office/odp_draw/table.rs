//! Tables: a `draw:frame` holding a `table:table` becomes cell shapes (fill and text) and border
//! lines of the scene.
//!
//! # Geometry
//!
//! * The table sits at the frame's top-left corner. Its columns are the `table:table-column`
//!   widths (`style:column-width` of the column's style, `table:number-columns-repeated` expands
//!   them) and its rows the `table:table-row` heights (`style:row-height` or
//!   `style:min-row-height` of the row's style) -- not the frame's extent, which LibreOffice keeps
//!   in step with them. Columns that state no width (or a width of 0) share the width the frame has left, equally.
//! * A row **grows** to fit its tallest cell: the cell's text is laid out (the same layout the
//!   renderer will use) at the cell's text width and the row becomes at least text height + top
//!   and bottom padding. A cell spanning several rows adds what is missing to the last row it spans.
//!   The table can end below its frame; nothing below it moves. Cells with vertical text do not
//!   grow their row.
//! * Merges: `table:number-columns-spanned` / `number-rows-spanned` on the origin cell,
//!   `table:covered-table-cell` for the slots it covers. The origin cell is drawn over the whole
//!   merged box; spans are clamped to the grid and to the slots an earlier cell already took. A
//!   `table:table-cell` that lands on a taken slot is dropped, a covered cell is never drawn.
//!
//! # Cell properties
//!
//! A cell's style is the `table-cell` style named by `table:style-name` of the cell, else by
//! `table:default-cell-style-name` of its row. Read from it (the layers of the style chain, the
//! strongest first): `draw:fill` / `draw:fill-color` / `draw:opacity` (LibreOffice stores a cell's
//! fill as graphic properties; a fill colour with no `draw:fill` is a solid fill, as in
//! LibreOffice's tables; an opacity outside 0..100 % is ignored), `fo:background-color` of
//! `style:table-cell-properties`, the borders `fo:border` / `fo:border-left` / `-right` / `-top`
//! / `-bottom` (of the paragraph properties, where Impress writes them, or of the table cell
//! properties), the padding (`fo:padding*`, 0.25 cm sideways and 0.125 cm above and below
//! otherwise), the vertical alignment (`draw:textarea-vertical-align` or `style:vertical-align`)
//! and `style:writing-mode`. The text properties and paragraph properties of the style are the
//! base of the cell's text (the paragraphs' own styles win).
//!
//! # Table templates
//!
//! `table:template-name` names a `table:table-template` of `styles.xml` whose parts
//! (`table:body`, `table:odd-rows` / `even-rows`, `odd-columns` / `even-columns`, `first-row`,
//! `last-row`, `first-column`, `last-column`) name `table-cell` styles. The parts that
//! the table's `table:use-*-styles` flags switch on lie under the cell's own style, weakest first:
//! the template's `table:background`, the body, the banded rows (counted from the first body row,
//! which is *odd*), the banded columns, the first / last column, the first / last row. A cell
//! with a style of its own overrides them property by property.
//!
//! # Borders
//!
//! Every edge between two cells has up to two candidates (the border of the cell above or on the
//! left, and of the one below or on the right). **The wider line wins; a border that is `none`
//! never overrides a line**, and on a tie the later cell's wins. Edges inside a merged cell are
//! not drawn. Equal neighbouring segments are drawn as one line, with square caps so that corners
//! close. A `double` border is drawn as a compound line; `dotted` and `dashed` as dashes.
//!
//! # Budgets
//!
//! [`MAX_TABLE_ROWS`], [`MAX_TABLE_COLS`], [`MAX_TABLE_CELLS`] (rows x columns), the items of the
//! slide (`DocOptions::max_slide_shapes`) and the characters of the slide. Going over one drops
//! the rest and sets `truncated`.

use crate::preview::office::slide_draw as sd;
use sd::Rgba;

use super::body::Env;
use super::styles::{StyleBook, View};
use super::units::{color, pct};
use super::*;

/// Most rows of a table that are drawn.
pub(super) const MAX_TABLE_ROWS: usize = 500;
/// Most columns of a table that are drawn.
pub(super) const MAX_TABLE_COLS: usize = 100;
/// Most cells (rows x columns) of a table that are drawn; rows past it are dropped.
pub(super) const MAX_TABLE_CELLS: usize = 5_000;
/// Deepest nesting of row / column groups followed.
const MAX_GROUP_NEST: usize = 6;
/// Largest length read from a table (EMU).
const MAX_EMU: f64 = 4.0e9;
/// The widest `number-*-repeated` count believed (a repeat past the grid is clamped to it anyway;
/// this keeps the arithmetic far from overflow).
const MAX_REPEAT: usize = 1_000_000;
/// Width of a border that states none (1 pt).
const DEFAULT_BORDER_W: f64 = 12_700.0;

const NONE: usize = usize::MAX;
/// The four sides in the order the arrays of a cell hold them.
const SIDES: [&str; 4] = ["left", "right", "top", "bottom"];
const LEFT: usize = 0;
const RIGHT: usize = 1;
const TOP: usize = 2;
const BOTTOM: usize = 3;

/// One cell that owns a box of the grid.
struct Cell<'a> {
    node: &'a Node,
    r: usize,
    c: usize,
    rs: usize,
    cs: usize,
    view: View<'a>,
    fill: sd::Fill,
    /// Left, right, top, bottom; `None` is no line.
    borders: [Option<sd::Line>; 4],
    body: Option<sd::TextBody>,
    /// The height the text asks for (EMU).
    need: f64,
}

/// What the table flags switch on.
#[derive(Clone, Copy, Default)]
struct Flags {
    first_row: bool,
    last_row: bool,
    first_col: bool,
    last_col: bool,
    band_rows: bool,
    band_cols: bool,
}

/// The leaves named `leaf` under `n`, in document order, looking through the grouping elements
/// `containers`.
fn collect<'n>(
    n: &'n Node,
    containers: &[&str],
    leaf: &str,
    depth: usize,
    out: &mut Vec<&'n Node>,
    over: &mut bool,
) {
    for k in n.nodes() {
        if k.name == leaf {
            if out.len() >= MAX_TABLE_ROWS.max(MAX_TABLE_COLS) * 4 {
                *over = true;
                return;
            }
            out.push(k);
        } else if containers.contains(&k.name.as_str()) {
            if depth >= MAX_GROUP_NEST {
                *over = true;
            } else {
                collect(k, containers, leaf, depth + 1, out, over);
            }
        }
    }
}

fn repeat(n: &Node, key: &str) -> usize {
    n.attr(key)
        .and_then(|v| v.trim().parse::<usize>().ok())
        .map_or(1, |v| v.clamp(1, MAX_REPEAT))
}

fn len_prop(v: &View, element: &str, key: &str) -> Option<f64> {
    v.x(element, key)
        .and_then(emu)
        .map(|e| e.clamp(0.0, MAX_EMU))
}

/// One border value (`0.28pt solid #808080`, `none`) of a style: `Some(None)` when it says there
/// is no line.
fn parse_border(v: &str) -> Option<Option<sd::Line>> {
    let v = v.trim();
    if v.is_empty() {
        return None;
    }
    let mut width = None;
    let mut style = "solid";
    let mut col = None;
    for t in v.split_whitespace() {
        if let Some(c) = color(t) {
            col = Some(c);
        } else if let Some(w) = emu(t) {
            width = Some(w.abs().min(1.0e8));
        } else {
            style = t;
        }
    }
    if matches!(style, "none" | "hidden") {
        return Some(None);
    }
    let w = width.unwrap_or(DEFAULT_BORDER_W);
    if w <= 0.0 {
        return Some(None);
    }
    let mut line = sd::Line::solid(w, col.unwrap_or(Rgba::BLACK));
    match style {
        "double" => line.compound = sd::Compound::Dbl,
        "dotted" => line.dash = sd::Dash::SysDot,
        "dashed" => line.dash = sd::Dash::Dash,
        _ => {}
    }
    Some(Some(line))
}

/// The border of `side` the layers of `v` state (strongest layer first; within a layer the side's
/// own property beats `fo:border`).
fn border_of(v: &View, side: &str) -> Option<sd::Line> {
    let key = format!("border-{side}");
    for l in v.layers().iter().rev() {
        let tcp = l
            .extra
            .iter()
            .find(|(e, _)| e == "table-cell-properties")
            .map(|(_, a)| a);
        for attrs in [Some(&l.para), tcp].into_iter().flatten() {
            for k in [key.as_str(), "border"] {
                if let Some((_, val)) = attrs.iter().find(|(n, _)| n == k) {
                    if let Some(b) = parse_border(val) {
                        return b;
                    }
                }
            }
        }
    }
    None
}

/// Adds the cell style `name` to `v`. A name that is no `table-cell` style (Impress writes
/// `table:default-cell-style-name="standard"`, the drawing style of that name) is looked up among
/// the graphic styles, whose fill the cell then has, as in LibreOffice.
fn push_cell_style<'a>(v: &mut View<'a>, book: &'a StyleBook, name: &str) {
    if book.style("table-cell", name).is_some() {
        v.push_chain(book, "table-cell", name);
    } else {
        v.push_chain(book, "graphic", name);
    }
}

/// The wider of two candidate lines (the later one on a tie); a missing one never wins.
fn resolve(a: Option<&sd::Line>, b: Option<&sd::Line>) -> Option<sd::Line> {
    match (a, b) {
        (None, None) => None,
        (Some(x), None) | (None, Some(x)) => Some(x.clone()),
        (Some(x), Some(y)) => Some(if x.width > y.width {
            x.clone()
        } else {
            y.clone()
        }),
    }
}

impl<'a> Sb<'a> {
    /// The style view of a cell: the template's parts the flags switch on, the row's default
    /// style, then the cell's own style.
    #[allow(clippy::too_many_arguments)]
    fn cell_view(
        &self,
        tpl: Option<&'a Node>,
        flags: Flags,
        pos: (usize, usize),
        dims: (usize, usize),
        row_style: Option<&'a str>,
        own: Option<&'a str>,
    ) -> View<'a> {
        let book = self.book;
        let mut v = View::with_default(book, "table-cell");
        let (r, c) = pos;
        let (nr, nc) = dims;
        let part = |v: &mut View<'a>, name: &str| {
            if let Some(s) = tpl
                .and_then(|t| t.nodes().find(|k| k.name == name))
                .and_then(|k| qattr(k, "table:style-name"))
            {
                push_cell_style(v, book, s);
            }
        };
        if tpl.is_some() {
            part(&mut v, "background");
            part(&mut v, "body");
            let first = usize::from(flags.first_row);
            if flags.band_rows && r >= first && !(flags.last_row && r + 1 == nr) {
                part(
                    &mut v,
                    if (r - first) % 2 == 0 {
                        "odd-rows"
                    } else {
                        "even-rows"
                    },
                );
            }
            let first = usize::from(flags.first_col);
            if flags.band_cols && c >= first && !(flags.last_col && c + 1 == nc) {
                part(
                    &mut v,
                    if (c - first) % 2 == 0 {
                        "odd-columns"
                    } else {
                        "even-columns"
                    },
                );
            }
            if flags.first_col && c == 0 {
                part(&mut v, "first-column");
            }
            if flags.last_col && c + 1 == nc {
                part(&mut v, "last-column");
            }
            if flags.first_row && r == 0 {
                part(&mut v, "first-row");
            }
            if flags.last_row && r + 1 == nr {
                part(&mut v, "last-row");
            }
        }
        if let Some(s) = row_style {
            push_cell_style(&mut v, book, s);
        }
        if let Some(s) = own {
            push_cell_style(&mut v, book, s);
        }
        v
    }

    /// The fill of a cell.
    fn cell_fill(&mut self, v: &View, w: f64, h: f64) -> sd::Fill {
        if v.g("fill").is_some() {
            return self.fill(v, w, h).unwrap_or(sd::Fill::None);
        }
        // LibreOffice's table cells are solid unless they say otherwise.
        let alpha = v
            .g("opacity")
            .and_then(pct)
            .filter(|a| (0.0..=1.0).contains(a))
            .unwrap_or(1.0);
        if let Some(c) = v.g("fill-color").and_then(color) {
            return sd::Fill::Solid(c.with_alpha(alpha));
        }
        match v.x("table-cell-properties", "background-color") {
            Some(b) => color(b).map_or(sd::Fill::None, sd::Fill::Solid),
            None => sd::Fill::None,
        }
    }

    /// The text of a cell as a body with the cell's padding, anchor and direction.
    fn cell_body(
        &mut self,
        frame: &'a Node,
        node: &'a Node,
        view: &View<'a>,
        fill: &sd::Fill,
    ) -> Option<sd::TextBody> {
        let env = Env {
            pres: qattr(frame, "presentation:style-name"),
            gfx: qattr(frame, "draw:style-name"),
            cell: Some(view.clone()),
            auto: Some(self.auto_color(fill)),
            ..Env::default()
        };
        let mut body = self.text_body(&node.kids, &env, view)?;
        let pad = |k: &str, d: f64| {
            view.g(k)
                .or_else(|| view.x("table-cell-properties", k))
                .or_else(|| {
                    view.g("padding")
                        .or_else(|| view.x("table-cell-properties", "padding"))
                })
                .and_then(emu)
                .unwrap_or(d)
                .clamp(0.0, 1.0e8)
        };
        body.insets = (
            pad("padding-left", 90_000.0),
            pad("padding-top", 45_000.0),
            pad("padding-right", 90_000.0),
            pad("padding-bottom", 45_000.0),
        );
        let align = view
            .g("textarea-vertical-align")
            .or_else(|| view.x("table-cell-properties", "vertical-align"))
            .map_or("top", str::trim);
        body.anchor = match align {
            "middle" => sd::Anchor::Middle,
            "bottom" => sd::Anchor::Bottom,
            _ => sd::Anchor::Top,
        };
        if view
            .x("table-cell-properties", "writing-mode")
            .map(str::trim)
            .is_some_and(|m| matches!(m, "tb-rl" | "tb" | "tb-lr"))
        {
            body.vert = sd::Vert::EaVert;
        }
        body.wrap = true;
        body.autofit = sd::AutoFit::None;
        Some(body)
    }

    fn table_item(&mut self, out: &mut Vec<sd::Item>, item: sd::Item) -> bool {
        if self.items >= self.max_items {
            self.truncated = true;
            return false;
        }
        self.items += 1;
        out.push(item);
        true
    }

    /// A table as scene items (see the module documentation).
    pub(super) fn table_items(
        &mut self,
        frame: &'a Node,
        table: &'a Node,
        xf: sd::Xfrm,
        out: &mut Vec<sd::Item>,
    ) {
        let book = self.book;
        let mut over = false;
        // Columns.
        let mut col_nodes = Vec::new();
        collect(
            table,
            &[
                "table-columns",
                "table-header-columns",
                "table-column-group",
            ],
            "table-column",
            0,
            &mut col_nodes,
            &mut over,
        );
        let mut widths: Vec<f64> = Vec::new();
        'cols: for n in col_nodes {
            let w = qattr(n, "table:style-name")
                .map(|s| {
                    let mut v = View::default();
                    v.push_chain(book, "table-column", s);
                    len_prop(&v, "table-column-properties", "column-width").unwrap_or(0.0)
                })
                .unwrap_or(0.0);
            for _ in 0..repeat(n, "number-columns-repeated") {
                if widths.len() >= MAX_TABLE_COLS {
                    over = true;
                    break 'cols;
                }
                widths.push(w);
            }
        }
        // Rows.
        let mut row_nodes = Vec::new();
        collect(
            table,
            &["table-rows", "table-header-rows", "table-row-group"],
            "table-row",
            0,
            &mut row_nodes,
            &mut over,
        );
        let mut rows: Vec<&'a Node> = Vec::new();
        'rows: for n in row_nodes {
            for _ in 0..repeat(n, "number-rows-repeated") {
                if rows.len() >= MAX_TABLE_ROWS {
                    over = true;
                    break 'rows;
                }
                rows.push(n);
            }
        }
        if widths.is_empty() {
            // No column declared: as many as the widest row has cells.
            let most = rows
                .iter()
                .map(|r| {
                    r.nodes()
                        .filter(|k| matches!(k.name.as_str(), "table-cell" | "covered-table-cell"))
                        .map(|k| repeat(k, "number-columns-repeated"))
                        .sum::<usize>()
                })
                .max()
                .unwrap_or(0)
                .min(MAX_TABLE_COLS);
            widths = vec![0.0; most];
        }
        let nc = widths.len();
        if nc == 0 || rows.is_empty() {
            return;
        }
        let max_rows = MAX_TABLE_ROWS.min(MAX_TABLE_CELLS / nc);
        if rows.len() > max_rows {
            rows.truncate(max_rows);
            over = true;
        }
        let nr = rows.len();
        // Columns with no width (`style:use-optimal-column-width`) share what the frame has left.
        let known: f64 = widths.iter().sum();
        let unset = widths.iter().filter(|w| **w <= 0.0).count();
        if unset > 0 && xf.w > known {
            let each = (xf.w - known) / unset as f64;
            for w in widths.iter_mut().filter(|w| **w <= 0.0) {
                *w = each;
            }
        }
        let mut heights: Vec<f64> = rows
            .iter()
            .map(|r| {
                qattr(r, "table:style-name")
                    .map(|s| {
                        let mut v = View::default();
                        v.push_chain(book, "table-row", s);
                        len_prop(&v, "table-row-properties", "row-height")
                            .or_else(|| len_prop(&v, "table-row-properties", "min-row-height"))
                            .unwrap_or(0.0)
                    })
                    .unwrap_or(0.0)
            })
            .collect();

        // The template and its flags.
        let flag = |k: &str| table.attr(k).is_some_and(|v| v.trim() == "true");
        let flags = Flags {
            first_row: flag("use-first-row-styles"),
            last_row: flag("use-last-row-styles"),
            first_col: flag("use-first-column-styles"),
            last_col: flag("use-last-column-styles"),
            band_rows: flag("use-banding-rows-styles"),
            band_cols: flag("use-banding-columns-styles"),
        };
        let tpl = table
            .attr("template-name")
            .and_then(|n| book.templates.get(n.trim()));

        // The cells: every slot that no earlier cell covers and that has a `table:table-cell`
        // starts one, with its spans clamped to the grid and to the free slots.
        let mut owner = vec![NONE; nr * nc];
        let mut cells: Vec<Cell<'a>> = Vec::new();
        for (r, row) in rows.iter().enumerate() {
            let row_style = qattr(row, "table:default-cell-style-name");
            let mut c = 0usize;
            for k in row.nodes() {
                let covered = match k.name.as_str() {
                    "table-cell" => false,
                    "covered-table-cell" => true,
                    _ => continue,
                };
                for _ in 0..repeat(k, "number-columns-repeated") {
                    if covered {
                        // (The slot belongs to the cell that spans it.)
                        c += 1;
                        continue;
                    }
                    // A cell on a slot another cell took moves on to the next free one.
                    while c < nc && owner[r * nc + c] != NONE {
                        c += 1;
                    }
                    if c >= nc {
                        over = true;
                        break;
                    }
                    let span = |key: &str| {
                        k.attr(key)
                            .and_then(|v| v.trim().parse::<usize>().ok())
                            .map_or(1, |v| v.clamp(1, MAX_REPEAT))
                    };
                    let mut cs = span("number-columns-spanned").min(nc - c);
                    while cs > 1 && (c..c + cs).any(|cc| owner[r * nc + cc] != NONE) {
                        cs -= 1;
                    }
                    let mut rs = 1;
                    let want = span("number-rows-spanned").min(nr - r);
                    while rs < want && (c..c + cs).all(|cc| owner[(r + rs) * nc + cc] == NONE) {
                        rs += 1;
                    }
                    let idx = cells.len();
                    for rr in r..r + rs {
                        for cc in c..c + cs {
                            owner[rr * nc + cc] = idx;
                        }
                    }
                    let view = self.cell_view(
                        tpl,
                        flags,
                        (r, c),
                        (nr, nc),
                        row_style,
                        qattr(k, "table:style-name"),
                    );
                    cells.push(Cell {
                        node: k,
                        r,
                        c,
                        rs,
                        cs,
                        view,
                        fill: sd::Fill::None,
                        borders: [None, None, None, None],
                        body: None,
                        need: 0.0,
                    });
                    // (The covered cells that follow step over the rest of the span.)
                    c += 1;
                }
            }
        }

        // Fills, borders and text of every cell, with the height the text asks for.
        #[allow(clippy::needless_range_loop)] // (`self` is borrowed again inside the loop)
        for i in 0..cells.len() {
            let (node, view, c, cs) = {
                let k = &cells[i];
                (k.node, k.view.clone(), k.c, k.cs)
            };
            let width: f64 = widths[c..c + cs].iter().sum();
            let fill = self.cell_fill(&view, width, 0.0);
            let borders = SIDES.map(|s| border_of(&view, s));
            let body = self.cell_body(frame, node, &view, &fill);
            let mut need = 0.0;
            if let Some(b) = &body {
                if b.vert == sd::Vert::Horz && !b.paragraphs.is_empty() {
                    let (l, t, rr, bt) = b.insets;
                    let lay =
                        sd::text::layout(b, ((width - l - rr).max(0.0)) / sd::EMU_PER_PX, 0.0);
                    if lay.truncated {
                        self.truncated = true;
                    }
                    need = t + bt + lay.content_h * sd::EMU_PER_PX;
                }
            }
            let k = &mut cells[i];
            k.fill = fill;
            k.borders = borders;
            k.body = body;
            k.need = need;
        }

        // Rows grow to what their cells need: single-row cells first, then the spanning ones.
        let mut order: Vec<usize> = (0..cells.len()).collect();
        order.sort_by_key(|&i| cells[i].rs);
        for i in order {
            let (r, rs, need) = (cells[i].r, cells[i].rs, cells[i].need);
            let have: f64 = heights[r..r + rs].iter().sum();
            if need > have {
                heights[r + rs - 1] += need - have;
            }
        }
        if over {
            self.truncated = true;
        }

        // Positions.
        let mut xs = vec![0.0; nc + 1];
        for c in 0..nc {
            xs[c + 1] = xs[c] + widths[c];
        }
        let mut ys = vec![0.0; nr + 1];
        for r in 0..nr {
            ys[r + 1] = ys[r] + heights[r];
        }

        // Fills and text.
        for k in &cells {
            if !k.fill.is_visible() && k.body.is_none() {
                continue;
            }
            let mut shape = sd::ShapeItem::new(
                sd::Xfrm::rect(
                    xf.x + xs[k.c],
                    xf.y + ys[k.r],
                    xs[k.c + k.cs] - xs[k.c],
                    ys[k.r + k.rs] - ys[k.r],
                ),
                sd::Geometry::Rect,
            );
            shape.fill = k.fill.clone();
            shape.text = k.body.clone();
            if !self.table_item(out, sd::Item::Shape(shape)) {
                return;
            }
        }

        // Borders: horizontal edges, then vertical ones, as runs of equal neighbouring segments.
        let line_item = |x: f64, y: f64, w: f64, h: f64, line: &sd::Line| {
            let mut line = line.clone();
            if line.dash == sd::Dash::Solid && line.cap == sd::Cap::Flat {
                line.cap = sd::Cap::Square;
            }
            let mut shape = sd::ShapeItem::new(sd::Xfrm::rect(x, y, w, h), sd::Geometry::Line);
            shape.line = Some(line);
            sd::Item::Shape(shape)
        };
        let cell_at = |r: usize, c: usize| &cells[owner[r * nc + c]];
        let has = |r: usize, c: usize| owner[r * nc + c] != NONE;
        #[allow(clippy::needless_range_loop)] // (`r` is also the row edge being drawn)
        for r in 0..=nr {
            let mut run: Option<(usize, sd::Line)> = None;
            for c in 0..=nc {
                let line = if c < nc {
                    let above = (r > 0 && has(r - 1, c)).then(|| cell_at(r - 1, c));
                    let below = (r < nr && has(r, c)).then(|| cell_at(r, c));
                    let same = matches!((above, below), (Some(a), Some(b)) if std::ptr::eq(a, b));
                    if same {
                        None
                    } else {
                        let a = above
                            .filter(|k| k.r + k.rs == r)
                            .and_then(|k| k.borders[BOTTOM].as_ref());
                        let b = below
                            .filter(|k| k.r == r)
                            .and_then(|k| k.borders[TOP].as_ref());
                        resolve(a, b)
                    }
                } else {
                    None
                };
                match (&mut run, line) {
                    (Some((_, l)), Some(n)) if *l == n => {}
                    (run_, n) => {
                        if let Some((c0, l)) = run_.take() {
                            let item =
                                line_item(xf.x + xs[c0], xf.y + ys[r], xs[c] - xs[c0], 0.0, &l);
                            if !self.table_item(out, item) {
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
                    let left = (c > 0 && has(r, c - 1)).then(|| cell_at(r, c - 1));
                    let right = (c < nc && has(r, c)).then(|| cell_at(r, c));
                    let same = matches!((left, right), (Some(a), Some(b)) if std::ptr::eq(a, b));
                    if same {
                        None
                    } else {
                        let a = left
                            .filter(|k| k.c + k.cs == c)
                            .and_then(|k| k.borders[RIGHT].as_ref());
                        let b = right
                            .filter(|k| k.c == c)
                            .and_then(|k| k.borders[LEFT].as_ref());
                        resolve(a, b)
                    }
                } else {
                    None
                };
                match (&mut run, line) {
                    (Some((_, l)), Some(n)) if *l == n => {}
                    (run_, n) => {
                        if let Some((r0, l)) = run_.take() {
                            let item =
                                line_item(xf.x + xs[c], xf.y + ys[r0], 0.0, ys[r] - ys[r0], &l);
                            if !self.table_item(out, item) {
                                return;
                            }
                        }
                        run = n.map(|n| (r, n));
                    }
                }
            }
        }
    }
}
