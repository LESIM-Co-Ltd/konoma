//! Boxes that hold a *table* rather than one centred label — stage 3's only new geometry.
//!
//! A flowchart node, a state and a note all say one thing, so they are drawn as a shape with a
//! label in the middle of it. A class and an entity say several things at once, in compartments
//! that a reader scans down: `+String owner` above `+deposit(amount)`, `string name` beside
//! `PK`. That is a small table, and a table cannot be described by a width and a height.
//!
//! So a node may carry a [`Panel`]: rows of cells, each with its own place inside the box, plus
//! the rules that separate them. It is **finished geometry** by the time it leaves this module —
//! every number is an offset from the box's top-left corner — which is what keeps it on the right
//! side of the seam (`docs/FEATURE-MERMAID-RENDERER.md` §6): [`super::svg`] only translates it,
//! and the invariant tests can state things about it without parsing an SVG.
//!
//! # Why the box is sized here and not in `shapes`
//!
//! [`super::shapes::size`] answers "how big a box does this label need?", which presumes there is
//! one label. A class box's width is decided by its widest *row* and its height by how many rows
//! there are, so the panel computes the box and `shapes::size` hands it back unchanged. Sizing it
//! anywhere else would mean two places deciding the same number — the shape that konoma's own
//! history says goes wrong quietly.
//!
//! # What is konoma's, and what is mermaid's
//!
//! The *structure* is mermaid's, read out of `classBox.ts` and `erBox.ts`: annotations above the
//! class name in the top compartment, attributes and then methods below it, a rule between each;
//! an entity's name band above a grid of `type | name | keys | comment` with a rule under the
//! name and a rule down each column boundary. The *numbers* are konoma's (§0-1), and two
//! behaviours are deliberately not mermaid's, because they cost a terminal reader space for
//! nothing:
//!
//! * mermaid draws **two** empty compartments under a class with no members at all (its
//!   `renderExtraBox`); konoma draws one, which is still unmistakably a UML class box.
//! * mermaid draws an empty methods compartment under a class that has attributes and no methods;
//!   konoma leaves it off.
//!
//! Both are pinned by tests so they read as decisions rather than as omissions.

use super::labels::{self, Label};
use super::shapes::Size;

/// Space between a class box's content and its outline, left and right.
pub const CLASS_PAD_X: f64 = 12.0;

/// Space above and below each compartment's rows.
pub const CLASS_PAD_Y: f64 = 6.0;

/// Space between an entity's cell text and its column's rules, per side.
pub const ER_CELL_PAD_X: f64 = 8.0;

/// Space above and below an entity's attribute row.
pub const ER_ROW_PAD_Y: f64 = 4.0;

/// Space above and below an entity's name.
pub const ER_NAME_PAD_Y: f64 = 6.0;

/// Space between an entity's name and its box, left and right.
pub const ER_NAME_PAD_X: f64 = 12.0;

/// The narrowest an entity box may be. mermaid's `er.minEntityWidth`, schema default 100 — kept
/// because an entity with no attributes is otherwise a small box in a diagram of large ones, and
/// a row of ragged boxes reads as noise.
pub const ER_MIN_WIDTH: f64 = 100.0;

/// One piece of text inside a panel.
#[derive(Debug, Clone, PartialEq)]
pub struct PanelCell {
    /// The measured text.
    pub label: Label,
    /// Offset from the box's **left** edge: the text's centre when [`centered`], its left edge
    /// otherwise.
    ///
    /// [`centered`]: PanelCell::centered
    pub x: f64,
    /// Whether the text is centred on `x` rather than starting at it.
    pub centered: bool,
}

/// One row of a panel.
#[derive(Debug, Clone, PartialEq)]
pub struct PanelRow {
    /// The cells, left to right.
    pub cells: Vec<PanelCell>,
    /// Offset of the row's vertical centre from the box's **top** edge.
    pub y: f64,
}

/// The contents of a box that holds a table.
#[derive(Debug, Clone, PartialEq)]
pub struct Panel {
    /// The rows, top to bottom.
    pub rows: Vec<PanelRow>,
    /// Horizontal rules, as offsets from the box's top edge. Each one spans the whole width.
    pub rules: Vec<f64>,
    /// Vertical rules, as offsets from the box's left edge. Each one runs from the first
    /// horizontal rule to the bottom of the box — an entity's column boundaries, and nothing else.
    pub columns: Vec<f64>,
    /// The box the panel fills, which is the size the node is laid out at.
    pub size: Size,
}

impl Panel {
    /// Every cell's rectangle, relative to the box's top-left corner, as `(l, t, r, b)`.
    ///
    /// The invariant tests use this to ask whether the text a reader sees is inside the box that
    /// is drawn around it, which is the one thing a panel can get wrong that nothing else checks.
    pub fn cell_bounds(&self) -> Vec<(f64, f64, f64, f64)> {
        let lh = labels::line_height();
        let mut out = Vec::new();
        for row in &self.rows {
            for cell in &row.cells {
                let w = cell.label.width;
                let l = if cell.centered {
                    cell.x - w / 2.0
                } else {
                    cell.x
                };
                out.push((l, row.y - lh / 2.0, l + w, row.y + lh / 2.0));
            }
        }
        out
    }
}

/// One compartment of a class box: some rows, all aligned the same way.
#[derive(Debug, Clone, PartialEq)]
pub struct Compartment {
    /// The lines, top to bottom. Each is one row.
    pub lines: Vec<Label>,
    /// Whether the lines are centred (the annotations and the class name) rather than
    /// left-aligned (the members and the methods).
    pub centered: bool,
}

/// Builds a class box: compartments stacked top to bottom with a rule between each pair.
///
/// An empty compartment still takes its padding and still gets its rule, which is how a class
/// with no members reads as "a class with no members" rather than as a plain box — see the module
/// docs for where konoma stops following mermaid on that.
pub fn class_panel(compartments: &[Compartment]) -> Panel {
    let lh = labels::line_height();
    let content_width = compartments
        .iter()
        .flat_map(|c| c.lines.iter())
        .map(|l| l.width)
        .fold(0.0_f64, f64::max);
    let width = content_width + CLASS_PAD_X * 2.0;

    let mut rows: Vec<PanelRow> = Vec::new();
    let mut rules: Vec<f64> = Vec::new();
    let mut y = 0.0_f64;
    for (i, compartment) in compartments.iter().enumerate() {
        if i > 0 {
            rules.push(y);
        }
        y += CLASS_PAD_Y;
        for line in &compartment.lines {
            rows.push(PanelRow {
                cells: vec![PanelCell {
                    label: line.clone(),
                    x: if compartment.centered {
                        width / 2.0
                    } else {
                        CLASS_PAD_X
                    },
                    centered: compartment.centered,
                }],
                y: y + lh / 2.0,
            });
            y += lh;
        }
        y += CLASS_PAD_Y;
    }

    Panel {
        rows,
        rules,
        columns: Vec::new(),
        size: Size::new(width, y),
    }
}

/// One attribute of an entity, already measured.
#[derive(Debug, Clone, PartialEq)]
pub struct AttributeRow {
    /// `string` in `string name PK "comment"`.
    pub kind: Label,
    /// `name`.
    pub name: Label,
    /// `PK`, `PK, FK`, … Blank when the attribute declares no key.
    pub keys: Label,
    /// The quoted comment. Blank when there is none.
    pub comment: Label,
}

impl AttributeRow {
    /// The four cells in column order.
    fn cells(&self) -> [&Label; 4] {
        [&self.kind, &self.name, &self.keys, &self.comment]
    }
}

/// Builds an entity box: a centred name band, a rule, then a grid of attributes.
///
/// The **keys and comment columns exist only if some attribute fills them** — mermaid's own rule
/// (`erBox.ts` drops a column whose widest cell is empty), and the one that keeps the common
/// `string name` entity from being drawn with two blank columns.
///
/// When the name is wider than the columns need, the surplus is shared out between the columns
/// that are present, so the vertical rules still reach the sides of the box. That is also
/// upstream's (`difference / totalWidthSections`).
pub fn er_panel(name: &Label, attributes: &[AttributeRow]) -> Panel {
    let lh = labels::line_height();
    let name_band = lh + ER_NAME_PAD_Y * 2.0;

    if attributes.is_empty() {
        let width = (name.width + ER_NAME_PAD_X * 2.0).max(ER_MIN_WIDTH);
        return Panel {
            rows: vec![PanelRow {
                cells: vec![PanelCell {
                    label: name.clone(),
                    x: width / 2.0,
                    centered: true,
                }],
                y: name_band / 2.0,
            }],
            rules: Vec::new(),
            columns: Vec::new(),
            size: Size::new(width, name_band),
        };
    }

    // Which of the four columns are present, and how wide each needs to be.
    let mut present = [true, true, false, false];
    let mut widths = [0.0_f64; 4];
    for a in attributes {
        for (i, cell) in a.cells().into_iter().enumerate() {
            if !cell.is_blank() {
                present[i] = true;
            }
            widths[i] = widths[i].max(cell.width);
        }
    }
    for i in 0..4 {
        widths[i] = if present[i] {
            widths[i] + ER_CELL_PAD_X * 2.0
        } else {
            0.0
        };
    }

    let columns_width: f64 = widths.iter().sum();
    let needed = (name.width + ER_NAME_PAD_X * 2.0).max(ER_MIN_WIDTH);
    if needed > columns_width {
        let shares = present.iter().filter(|p| **p).count() as f64;
        let each = (needed - columns_width) / shares;
        for i in 0..4 {
            if present[i] {
                widths[i] += each;
            }
        }
    }
    let width: f64 = widths.iter().sum();

    let mut rows = vec![PanelRow {
        cells: vec![PanelCell {
            label: name.clone(),
            x: width / 2.0,
            centered: true,
        }],
        y: name_band / 2.0,
    }];
    let mut rules = vec![name_band];
    let mut columns = Vec::new();
    let mut x = 0.0_f64;
    for i in 0..3 {
        x += widths[i];
        if present[i] && widths[i + 1..].iter().any(|w| *w > 0.0) {
            columns.push(x);
        }
    }

    let row_height = lh + ER_ROW_PAD_Y * 2.0;
    let mut y = name_band;
    for (i, a) in attributes.iter().enumerate() {
        if i > 0 {
            rules.push(y);
        }
        let mut left = 0.0_f64;
        let mut cells = Vec::new();
        for (c, cell) in a.cells().into_iter().enumerate() {
            if present[c] && !cell.is_blank() {
                cells.push(PanelCell {
                    label: cell.clone(),
                    x: left + ER_CELL_PAD_X,
                    centered: false,
                });
            }
            left += widths[c];
        }
        rows.push(PanelRow {
            cells,
            y: y + row_height / 2.0,
        });
        y += row_height;
    }

    Panel {
        rows,
        rules,
        columns,
        size: Size::new(width, y),
    }
}
