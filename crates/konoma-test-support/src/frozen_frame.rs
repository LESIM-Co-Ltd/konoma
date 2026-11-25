//! Compares the pre-extraction cell fixtures (`snapshots/project_*.json`, captured before the list
//! frame existed) with the current drawing. A list and a diff are drawn inside the old hub's outer
//! frame, so their body moved one column right and the tab row became the top edge. The fixtures
//! stay as they were; this module keeps their proof by mapping each frozen body cell to its new
//! place, and checks the cells the frozen fixture has no counterpart for against the frame rule.
//! Unframed content (text, document, loading) and cells outside the canvas stay one-to-one.
//!
//! One implementation for the host's tests (`src/e2e_tests/project_frozen_frame.rs`) and the
//! standalone app's tests (`crates/konoma-plugin/tests/app`). Callers pass the two body areas
//! (the old hub's and the framed one) so this crate does not depend on the plugin crate.
use ratatui::{
    buffer::{Buffer, Cell},
    layout::Rect,
};
use serde_json::{json, Value};

/// What a stage records about how it was drawn: whether the canvas shows a framed content, and the
/// overlay (a dialog, a menu or the help panel) drawn above the canvas, found as the bounding box
/// of the canvas cells that differ between the finished screen and the canvas drawn alone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameRecord {
    pub framed: bool,
    pub canvas: Rect,
    pub overlay: Option<Rect>,
}

impl FrameRecord {
    /// A stage whose content has no frame: every cell is compared one-to-one.
    pub fn unframed(canvas: Rect) -> Self {
        Self {
            framed: false,
            canvas,
            overlay: None,
        }
    }

    /// A framed canvas: the overlay is what differs between `finished` and `alone` inside it.
    pub fn framed(canvas: Rect, finished: &Buffer, alone: &Buffer) -> Self {
        Self {
            framed: true,
            canvas,
            overlay: overlay_of(finished, alone, canvas),
        }
    }
}

pub fn rect_json(r: Rect) -> Value {
    json!([r.x, r.y, r.width, r.height])
}

pub fn rect_of(value: &Value) -> Rect {
    let n: Vec<u16> = value
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n.as_u64().unwrap() as u16)
        .collect();
    Rect::new(n[0], n[1], n[2], n[3])
}

pub fn within(r: Rect, x: u16, y: u16) -> bool {
    x >= r.x && x < r.right() && y >= r.y && y < r.bottom()
}

/// The cell as a fixture records it: symbol, foreground, background, modifier bits.
pub fn cell_json(c: &Cell) -> Value {
    json!([
        c.symbol(),
        format!("{:?}", c.fg),
        format!("{:?}", c.bg),
        c.modifier.bits()
    ])
}

fn blank() -> Value {
    json!([" ", "Reset", "Reset", 0])
}

fn same(a: &Cell, b: &Cell) -> bool {
    (a.symbol(), a.fg, a.bg, a.modifier) == (b.symbol(), b.fg, b.bg, b.modifier)
}

/// The bounding box of the cells of `canvas` that differ between the finished screen and the
/// canvas drawn alone: where an overlay sits.
pub fn overlay_of(finished: &Buffer, alone: &Buffer, canvas: Rect) -> Option<Rect> {
    let mut bbox: Option<(u16, u16, u16, u16)> = None;
    for y in canvas.y..canvas.bottom() {
        for x in canvas.x..canvas.right() {
            if !same(&finished[(x, y)], &alone[(x, y)]) {
                bbox = Some(match bbox {
                    None => (x, y, x, y),
                    Some((x0, y0, x1, y1)) => (x0.min(x), y0.min(y), x1.max(x), y1.max(y)),
                });
            }
        }
    }
    bbox.map(|(x0, y0, x1, y1)| Rect::new(x0, y0, x1 - x0 + 1, y1 - y0 + 1))
}

/// The cells of a stage object (`rect`, `count`, `runs`) in row order.
pub fn grid_of_stage(stage: &Value) -> Vec<Value> {
    let mut cells = Vec::new();
    for run in stage["runs"].as_array().unwrap() {
        for _ in 0..run[0].as_u64().unwrap() {
            cells.push(run[1].clone());
        }
    }
    let r = rect_of(&stage["rect"]);
    assert_eq!(cells.len(), usize::from(r.width) * usize::from(r.height));
    cells
}

/// The cells of `region` of a buffer in row order.
pub fn grid_of_buffer(buffer: &Buffer, region: Rect) -> Vec<Value> {
    let mut cells = Vec::new();
    for y in region.y..region.bottom() {
        for x in region.x..region.right() {
            cells.push(cell_json(&buffer[(x, y)]));
        }
    }
    cells
}

/// The cells of a fixture kept as text rows plus runs of `[fg, bg, modifier]` styles (the whole
/// frame, one character per cell).
pub fn grid_of_rows(rows: &Value, styles: &Value, region: Rect) -> Vec<Value> {
    let mut style_cells: Vec<Value> = Vec::new();
    for run in styles.as_array().unwrap() {
        for _ in 0..run[0].as_u64().unwrap() {
            style_cells.push(run[1].clone());
        }
    }
    let mut cells = Vec::new();
    let rows = rows.as_array().unwrap();
    assert_eq!(rows.len(), usize::from(region.height), "rows of the frame");
    for row in rows {
        let symbols: Vec<String> = row.as_str().unwrap().chars().map(String::from).collect();
        assert_eq!(symbols.len(), usize::from(region.width), "row width");
        for symbol in symbols {
            let style = &style_cells[cells.len()];
            cells.push(json!([symbol, style[0], style[1], style[2]]));
        }
    }
    assert_eq!(cells.len(), style_cells.len(), "styles cover every cell");
    cells
}

/// A stage of the current drawing against its frozen counterpart: `new` and `old` are the cells of
/// `region` (row order). Framed content goes through [`assert_framed`], anything else is
/// one-to-one. `old_body` is the body of the old hub's drawing, `new_body` the inner rect of the
/// frame, both for this canvas.
pub fn assert_stage(
    path: &str,
    new: &[Value],
    old: &[Value],
    region: Rect,
    record: &FrameRecord,
    bodies: (Rect, Rect),
) {
    if !record.framed {
        assert_eq!(new, old, "{path}: unframed content is one-to-one");
        return;
    }
    assert_framed(path, new, old, region, record, bodies);
}

/// Framed content: see the module doc.
pub fn assert_framed(
    path: &str,
    new: &[Value],
    old: &[Value],
    region: Rect,
    record: &FrameRecord,
    (old_body, new_body): (Rect, Rect),
) {
    let canvas = record.canvas;
    let overlay = record.overlay;
    let covered = |x: u16, y: u16| overlay.is_some_and(|o| within(o, x, y));
    let at = |g: &[Value], x: u16, y: u16| -> Value {
        assert!(within(region, x, y), "{path}: {x},{y} outside the region");
        g[usize::from(y - region.y) * usize::from(region.width) + usize::from(x - region.x)].clone()
    };
    assert_eq!(
        new.len(),
        usize::from(region.width) * usize::from(region.height)
    );
    assert_eq!(old.len(), new.len());
    assert!(
        canvas.x >= region.x
            && canvas.y >= region.y
            && canvas.right() <= region.right()
            && canvas.bottom() <= region.bottom(),
        "{path}: the canvas lies in the frozen region"
    );
    // One-to-one outside the canvas.
    let mut outside = 0usize;
    for y in region.y..region.bottom() {
        for x in region.x..region.right() {
            if !within(canvas, x, y) {
                assert_eq!(at(new, x, y), at(old, x, y), "{path}: cell {x},{y} outside");
                outside += 1;
            }
        }
    }
    assert_eq!(
        outside,
        usize::from(region.width) * usize::from(region.height)
            - usize::from(canvas.width) * usize::from(canvas.height)
    );
    // The old body and the new inner rect: same size, the shift derived from their positions.
    assert_eq!(
        (old_body.width, old_body.height),
        (new_body.width, new_body.height)
    );
    assert_eq!(
        old_body.y, new_body.y,
        "the frame moves the body sideways only"
    );
    let dx = new_body.x - old_body.x;
    assert_eq!(dx, 1, "the frame moves the body one column right");
    let (mut overlay_cells, mut moved, mut lost) = (0usize, 0usize, 0usize);
    for y in old_body.y..old_body.bottom() {
        for x in old_body.x..old_body.right() {
            let frozen = at(old, x, y);
            if covered(x, y) {
                // Under the overlay both frames show the overlay at the same cell.
                assert_eq!(at(new, x, y), frozen, "{path}: overlay cell {x},{y}");
                overlay_cells += 1;
            } else if covered(x + dx, y) {
                // The overlay hides this body cell in the new frame: nothing visible may be lost.
                assert_eq!(
                    frozen,
                    blank(),
                    "{path}: body cell {x},{y} hidden behind the overlay"
                );
                lost += 1;
            } else {
                assert_eq!(at(new, x + dx, y), frozen, "{path}: body cell {x},{y}");
                moved += 1;
            }
        }
    }
    assert_eq!(
        overlay_cells + moved + lost,
        usize::from(old_body.width) * usize::from(old_body.height),
        "{path}: every frozen body cell is compared"
    );
    // Cells of the new inner rect the old frame showed under the overlay cannot be proven by it.
    for y in new_body.y..new_body.bottom() {
        for x in new_body.x..new_body.right() {
            if !covered(x, y) && covered(x - dx, y) {
                assert_eq!(at(new, x, y), blank(), "{path}: revealed cell {x},{y}");
            }
        }
    }
    // The frozen tab row (the old top row): the tabs are the leading styled cells.
    let tab_width = (canvas.x..canvas.right())
        .rev()
        .find(|x| at(old, *x, canvas.y) != blank())
        .map_or(0, |x| usize::from(x - canvas.x) + 1);
    assert!(
        tab_width > 0 && !covered(canvas.x, canvas.y),
        "{path}: tabs in the frozen row"
    );
    // The fixtures' lists have no title; the tabs sit left of the right corner when they fit.
    let tab_columns = crate::top_edge::tabs_columns_of_width(canvas, 0, tab_width as u16);
    for y in canvas.y..canvas.bottom() {
        for x in canvas.x..canvas.right() {
            if within(new_body, x, y) || covered(x, y) {
                continue;
            }
            let (left, right) = (x == canvas.x, x == canvas.right() - 1);
            let want = if y == canvas.y {
                if left {
                    json!(["┌", "Reset", "Reset", 0])
                } else if right {
                    json!(["┐", "Reset", "Reset", 0])
                } else if let Some(columns) = tab_columns.as_ref().filter(|c| c.contains(&x)) {
                    at(old, canvas.x + (x - columns.start), canvas.y)
                } else {
                    json!(["─", "Reset", "Reset", 0])
                }
            } else if y == canvas.bottom() - 1 {
                let glyph = if left {
                    "└"
                } else if right {
                    "┘"
                } else {
                    "─"
                };
                json!([glyph, "Reset", "Reset", 0])
            } else {
                json!(["│", "Reset", "Reset", 0])
            };
            assert_eq!(at(new, x, y), want, "{path}: frame cell {x},{y}");
        }
    }
    // The frozen cells of the canvas that were neither body nor tabs were blank.
    for y in canvas.y..canvas.bottom() {
        for x in canvas.x..canvas.right() {
            let in_tabs = y == canvas.y && usize::from(x - canvas.x) < tab_width;
            if !within(old_body, x, y) && !in_tabs && !covered(x, y) {
                assert_eq!(
                    at(old, x, y),
                    blank(),
                    "{path}: frozen cell {x},{y} was blank"
                );
            }
        }
    }
}
