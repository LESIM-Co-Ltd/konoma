//! Drawing a `treemap-beta`.
//!
//! # Area is the whole point, so area is what the layout preserves
//!
//! [`squarify`] is Bruls, Huizing and van Wijk's algorithm, and the property that matters is not
//! that the tiles look square — it is that **each tile's area is exactly its share of the rectangle
//! it was given**, at every level of the nesting. The crate konoma is replacing does not do this
//! at all: it draws nested boxes of arbitrary size, so a leaf worth 25 and one worth 12 come out
//! the same, which is the one comparison a treemap exists to make.
//!
//! # A tile is labelled only if the label fits inside it
//!
//! A name that overhangs its tile lands on the neighbouring one and reads as belonging to it.
//! Small tiles therefore carry no text, which is what the rest of the diagram — position and
//! area — is for.
//!
//! # Colour keys the section, not the tile
//!
//! Every tile takes the series colour of the **top-level** node it descends from, so a section
//! reads as one block and its internal divisions are told by the rules between them. Colouring per
//! tile would make a deep tree a mosaic in which nothing groups.

use crate::preview::mermaid::chart::treemap::{Node, Treemap};
use crate::preview::mermaid::layout::Point;

use super::super::labels::line_height;
use super::super::panel::{Panel, PanelCell, PanelRow};
use super::super::{normalise, Diagram, Label, PlacedNode, RenderError, Size, Theme};
use super::{add_title, bar_node, tick_text};

/// Width of the whole map.
pub const WIDTH: f64 = 480.0;

/// Height of the whole map.
pub const HEIGHT: f64 = 300.0;

/// Blank space inside a section's frame, before its children.
pub const SECTION_PAD: f64 = 3.0;

/// Blank space between a tile's edge and the text in it.
pub const TEXT_PAD: f64 = 5.0;

/// An axis-aligned rectangle, in px.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    /// Left edge.
    pub x: f64,
    /// Top edge.
    pub y: f64,
    /// Width.
    pub w: f64,
    /// Height.
    pub h: f64,
}

impl Rect {
    /// Its area.
    pub fn area(&self) -> f64 {
        self.w.max(0.0) * self.h.max(0.0)
    }
    /// Its centre.
    pub fn center(&self) -> Point {
        Point::new(self.x + self.w / 2.0, self.y + self.h / 2.0)
    }
}

/// Reads a mermaid treemap source and draws it.
///
/// **This is the entry point the golden tests go through** (§6).
pub fn render(code: &str, theme: &str) -> Result<String, RenderError> {
    let tm = crate::preview::mermaid::chart::treemap::parse(code)?;
    let diagram = lay_out(&tm)?;
    Ok(super::super::svg::emit(&diagram, &Theme::named(theme)))
}

/// Works out where every tile goes.
pub fn lay_out(tm: &Treemap) -> Result<Diagram, RenderError> {
    if !crate::preview::mermaid::text_metrics::fonts_available() {
        return Err(RenderError::NoFonts);
    }
    let total: f64 = tm.roots.iter().map(Node::total).sum();
    if total <= 0.0 {
        return Err(RenderError::ChartHasNoExtent {
            what: "every value is zero, so no tile has any area",
        });
    }
    let mut nodes: Vec<PlacedNode> = Vec::new();
    let rect = Rect {
        x: 0.0,
        y: 0.0,
        w: WIDTH,
        h: HEIGHT,
    };
    place(&tm.roots, rect, None, &mut nodes, &mut 0);

    let mut diagram = Diagram {
        nodes,
        ..Diagram::default()
    };
    add_title(&mut diagram, &tm.preamble);
    normalise(&mut diagram);
    Ok(diagram)
}

/// Lays `items` out inside `rect`, recursing into every section.
///
/// `series` is the top-level ancestor's colour, threaded down so a whole section is one colour;
/// `counter` numbers the tiles so their ids are unique whatever the shape of the tree.
fn place(
    items: &[Node],
    rect: Rect,
    series: Option<usize>,
    out: &mut Vec<PlacedNode>,
    counter: &mut usize,
) {
    let values: Vec<f64> = items.iter().map(Node::total).collect();
    let rects = squarify(&values, rect);
    for (i, (item, r)) in items.iter().zip(rects.iter()).enumerate() {
        // A tile with no area cannot be drawn; a zero-value datum is not a thin sliver, it is a
        // stroke that reads as a boundary between the two tiles either side of it.
        if r.w <= 0.5 || r.h <= 0.5 {
            continue;
        }
        let colour = series.unwrap_or(i);
        let id = *counter;
        *counter += 1;
        let mut tile = bar_node(
            format!("tile#{id}"),
            r.center(),
            Size::new(r.w, r.h),
            Some(colour),
        );
        if item.children.is_empty() {
            tile.panel = tile_panel(&item.name, item.value, *r, r.h);
            out.push(tile);
            continue;
        }
        // A section: its name goes in a band across the top, and its children fill what is left.
        let band = (line_height() + TEXT_PAD).min(r.h);
        tile.panel = tile_panel(&item.name, None, *r, band);
        out.push(tile);
        let inner = Rect {
            x: r.x + SECTION_PAD,
            y: r.y + band,
            w: (r.w - SECTION_PAD * 2.0).max(0.0),
            h: (r.h - band - SECTION_PAD).max(0.0),
        };
        if inner.w > 1.0 && inner.h > 1.0 {
            place(&item.children, inner, Some(colour), out, counter);
        }
    }
}

/// The name (and value) drawn inside a tile, or `None` when they would not fit.
///
/// The check is against the room the text is allowed — `fit_h`, which for a leaf is the whole tile
/// and for a section is only the band above its children. The panel's own size is always the
/// **tile's**, because `check_panels_stay_inside_their_box` states that a panel and its node are
/// the same box, and a panel claiming to be the band would be a second opinion about how big the
/// node is (`panel`'s module docs).
///
/// A label that overhangs lands on the neighbouring tile and reads as belonging to it, which is
/// worse than no label at all.
fn tile_panel(name: &str, value: Option<f64>, r: Rect, fit_h: f64) -> Option<Panel> {
    let name = Label::measure(name);
    let value = value.map(|v| Label::measure(&tick_text(v)));
    let rows = 1 + usize::from(value.is_some());
    let needed_h = rows as f64 * line_height() + TEXT_PAD;
    let needed_w = name.width.max(value.as_ref().map_or(0.0, |v| v.width)) + TEXT_PAD * 2.0;
    if needed_w > r.w || needed_h > fit_h || name.is_blank() {
        return None;
    }
    let mut panel_rows = vec![PanelRow {
        cells: vec![PanelCell {
            label: name,
            x: TEXT_PAD,
            centered: false,
        }],
        y: TEXT_PAD + line_height() / 2.0,
    }];
    if let Some(v) = value {
        panel_rows.push(PanelRow {
            cells: vec![PanelCell {
                label: v,
                x: TEXT_PAD,
                centered: false,
            }],
            y: TEXT_PAD + line_height() * 1.5,
        });
    }
    Some(Panel {
        rows: panel_rows,
        rules: Vec::new(),
        columns: Vec::new(),
        size: Size::new(r.w, r.h),
    })
}

/// The squarified treemap layout.
///
/// Items are laid into the rectangle in order, gathered into rows across whichever side is
/// currently shorter, and a row is closed as soon as adding the next item would make its worst
/// aspect ratio worse. The invariant the tests state is the one that matters: **the areas come out
/// in the ratio the values went in**, whatever shape the rectangle is.
pub fn squarify(values: &[f64], rect: Rect) -> Vec<Rect> {
    let total: f64 = values.iter().filter(|v| **v > 0.0).sum();
    let mut out = vec![
        Rect {
            x: rect.x,
            y: rect.y,
            w: 0.0,
            h: 0.0
        };
        values.len()
    ];
    if total <= 0.0 || rect.area() <= 0.0 {
        return out;
    }
    // Only positive values take part; a zero has no area to give.
    let live: Vec<usize> = (0..values.len()).filter(|i| values[*i] > 0.0).collect();
    let areas: Vec<f64> = live
        .iter()
        .map(|i| values[*i] / total * rect.area())
        .collect();

    let mut remaining = rect;
    let mut i = 0;
    while i < areas.len() {
        let short = remaining.w.min(remaining.h);
        if short <= 0.0 {
            break;
        }
        let mut end = i + 1;
        let mut best = worst(&areas[i..end], short);
        while end < areas.len() {
            let cand = worst(&areas[i..end + 1], short);
            if cand > best {
                break;
            }
            best = cand;
            end += 1;
        }
        let sum: f64 = areas[i..end].iter().sum();
        if remaining.w >= remaining.h {
            // A vertical strip down the left of what is left.
            let strip = (sum / remaining.h).min(remaining.w);
            let mut y = remaining.y;
            for (k, a) in areas[i..end].iter().enumerate() {
                let h = if strip > 0.0 { a / strip } else { 0.0 };
                out[live[i + k]] = Rect {
                    x: remaining.x,
                    y,
                    w: strip,
                    h,
                };
                y += h;
            }
            remaining = Rect {
                x: remaining.x + strip,
                y: remaining.y,
                w: remaining.w - strip,
                h: remaining.h,
            };
        } else {
            // A horizontal strip across the top.
            let strip = (sum / remaining.w).min(remaining.h);
            let mut x = remaining.x;
            for (k, a) in areas[i..end].iter().enumerate() {
                let w = if strip > 0.0 { a / strip } else { 0.0 };
                out[live[i + k]] = Rect {
                    x,
                    y: remaining.y,
                    w,
                    h: strip,
                };
                x += w;
            }
            remaining = Rect {
                x: remaining.x,
                y: remaining.y + strip,
                w: remaining.w,
                h: remaining.h - strip,
            };
        }
        i = end;
    }
    out
}

/// The worst aspect ratio in a row of `areas` laid along a side of length `side`.
fn worst(areas: &[f64], side: f64) -> f64 {
    let sum: f64 = areas.iter().sum();
    if sum <= 0.0 || side <= 0.0 {
        return f64::INFINITY;
    }
    let max = areas.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let min = areas.iter().copied().fold(f64::INFINITY, f64::min);
    let s2 = sum * sum;
    let w2 = side * side;
    (w2 * max / s2).max(s2 / (w2 * min))
}
