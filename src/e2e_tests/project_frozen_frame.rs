//! Compares the pre-extraction cell fixtures (`snapshots/project_*.json`, captured before the list
//! frame existed) with the current drawing. A list and a diff are drawn inside the old hub's outer
//! frame, so their body moved one column right and the tab row became the top edge. The fixtures
//! stay as they were; this module keeps their proof by mapping each frozen body cell to its new
//! place, and checks the cells the frozen fixture has no counterpart for against the frame rule.
//! Unframed content (text, document, loading) and cells outside the canvas stay one-to-one.
use super::*;
use konoma_plugin::host::project::draw::{project_body_area, unframed_body_area};
use konoma_plugin::host::project::ProjectContent;
use ratatui::{backend::TestBackend, buffer::Buffer, layout::Rect, Terminal};
use serde_json::{json, Value};

fn rect_json(r: Rect) -> Value {
    json!([r.x, r.y, r.width, r.height])
}

fn rect_of(value: &Value) -> Rect {
    let n: Vec<u16> = value
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n.as_u64().unwrap() as u16)
        .collect();
    Rect::new(n[0], n[1], n[2], n[3])
}

fn within(r: Rect, x: u16, y: u16) -> bool {
    x >= r.x && x < r.right() && y >= r.y && y < r.bottom()
}

/// What a stage records about how it was drawn: whether the canvas shows a framed content, and the
/// overlay (a dialog or the help panel) drawn above the canvas, found as the bounding box of the
/// canvas cells that differ between the finished screen and the canvas drawn alone.
pub(super) fn frame_info(sim: &mut Sim, canvas: Rect, finished: &Buffer) -> Value {
    let framed = sim.app.project_active()
        && matches!(
            sim.app.project_content().as_deref(),
            Some(ProjectContent::List(_) | ProjectContent::Diff(_))
        );
    if !framed {
        return json!({"framed": false});
    }
    let mut terminal =
        Terminal::new(TestBackend::new(finished.area.width, finished.area.height)).unwrap();
    let alone = terminal
        .draw(|f| crate::ui::project::render(f, &mut sim.app, canvas))
        .unwrap()
        .buffer
        .clone();
    let mut bbox: Option<(u16, u16, u16, u16)> = None;
    for y in canvas.y..canvas.bottom() {
        for x in canvas.x..canvas.right() {
            let (a, b) = (&finished[(x, y)], &alone[(x, y)]);
            if (a.symbol(), a.fg, a.bg, a.modifier) != (b.symbol(), b.fg, b.bg, b.modifier) {
                bbox = Some(match bbox {
                    None => (x, y, x, y),
                    Some((x0, y0, x1, y1)) => (x0.min(x), y0.min(y), x1.max(x), y1.max(y)),
                });
            }
        }
    }
    let overlay =
        bbox.map(|(x0, y0, x1, y1)| rect_json(Rect::new(x0, y0, x1 - x0 + 1, y1 - y0 + 1)));
    json!({"framed": true, "canvas": rect_json(canvas), "overlay": overlay})
}

fn blank() -> Value {
    json!([" ", "Reset", "Reset", 0])
}

fn grid(stage: &Value) -> Vec<Value> {
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

/// Equality of an actual value tree with a frozen one: stage objects (those with `runs`) go through
/// [`stage_eq`]; everything else must be equal. The `frame` key of an actual stage is the record
/// `frame_info` made and is not part of the frozen data.
pub(super) fn frozen_eq(actual: &Value, expected: &Value, path: &str) {
    match (actual, expected) {
        (Value::Object(a), Value::Object(e)) => {
            if e.contains_key("runs") {
                return stage_eq(a, e, path);
            }
            let keys = |m: &serde_json::Map<String, Value>| {
                m.keys()
                    .filter(|k| *k != "frame")
                    .cloned()
                    .collect::<Vec<_>>()
            };
            assert_eq!(keys(a), keys(e), "{path}: keys");
            for (key, value) in e {
                frozen_eq(&a[key], value, &format!("{path}/{key}"));
            }
        }
        (Value::Array(a), Value::Array(e)) => {
            assert_eq!(a.len(), e.len(), "{path}: length");
            for (i, (a, e)) in a.iter().zip(e).enumerate() {
                frozen_eq(a, e, &format!("{path}[{i}]"));
            }
        }
        _ => assert_eq!(actual, expected, "{path}"),
    }
}

fn stage_eq(
    actual: &serde_json::Map<String, Value>,
    expected: &serde_json::Map<String, Value>,
    path: &str,
) {
    let info = actual.get("frame").cloned().unwrap_or(Value::Null);
    for (key, value) in expected {
        if key != "runs" {
            assert_eq!(&actual[key], value, "{path}: {key}");
        }
    }
    let actual_keys: Vec<_> = actual.keys().filter(|k| *k != "frame").collect();
    assert_eq!(actual_keys.len(), expected.len(), "{path}: keys");
    let (a, e) = (
        Value::Object(actual.clone()),
        Value::Object(expected.clone()),
    );
    if info["framed"] != json!(true) {
        assert_eq!(
            a["runs"], e["runs"],
            "{path}: unframed content is one-to-one"
        );
        return;
    }
    let region = rect_of(&e["rect"]);
    let (new, old) = (grid(&a), grid(&e));
    let canvas = rect_of(&info["canvas"]);
    let overlay = (!info["overlay"].is_null()).then(|| rect_of(&info["overlay"]));
    let covered = |x: u16, y: u16| overlay.is_some_and(|o| within(o, x, y));
    let at = |g: &Vec<Value>, x: u16, y: u16| -> Value {
        assert!(within(region, x, y), "{path}: {x},{y} outside the region");
        g[usize::from(y - region.y) * usize::from(region.width) + usize::from(x - region.x)].clone()
    };
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
                assert_eq!(
                    at(&new, x, y),
                    at(&old, x, y),
                    "{path}: cell {x},{y} outside"
                );
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
    let (old_body, new_body) = (unframed_body_area(canvas), project_body_area(canvas));
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
            let frozen = at(&old, x, y);
            if covered(x, y) {
                // Under the overlay both frames show the overlay at the same cell.
                assert_eq!(at(&new, x, y), frozen, "{path}: overlay cell {x},{y}");
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
                assert_eq!(at(&new, x + dx, y), frozen, "{path}: body cell {x},{y}");
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
                assert_eq!(at(&new, x, y), blank(), "{path}: revealed cell {x},{y}");
            }
        }
    }
    // The frozen tab row (the old top row): the tabs are the leading styled cells.
    let tab_width = (canvas.x..canvas.right())
        .rev()
        .find(|x| at(&old, *x, canvas.y) != blank())
        .map_or(0, |x| usize::from(x - canvas.x) + 1);
    assert!(
        tab_width > 0 && !covered(canvas.x, canvas.y),
        "{path}: tabs in the frozen row"
    );
    // The fixtures' lists have no title; the tabs sit left of the right corner when they fit.
    let tab_columns =
        crate::test_support::top_edge::tabs_columns_of_width(canvas, 0, tab_width as u16);
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
                    at(&old, canvas.x + (x - columns.start), canvas.y)
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
            assert_eq!(at(&new, x, y), want, "{path}: frame cell {x},{y}");
        }
    }
    // The frozen cells of the canvas that were neither body nor tabs were blank.
    for y in canvas.y..canvas.bottom() {
        for x in canvas.x..canvas.right() {
            let in_tabs = y == canvas.y && usize::from(x - canvas.x) < tab_width;
            if !within(old_body, x, y) && !in_tabs && !covered(x, y) {
                assert_eq!(
                    at(&old, x, y),
                    blank(),
                    "{path}: frozen cell {x},{y} was blank"
                );
            }
        }
    }
}
