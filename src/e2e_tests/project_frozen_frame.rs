//! The host's side of the frozen-fixture comparison for framed project content: it records how a
//! stage was drawn (`frame_info`) and walks the value trees (`frozen_eq`). The mapping of each
//! frozen body cell and the frame rule are `konoma_test_support::frozen_frame`, shared with the
//! standalone app's tests.
use super::*;
use konoma_plugin::host::project::draw::{project_body_area, unframed_body_area};
use konoma_plugin::host::project::ProjectContent;
use konoma_test_support::frozen_frame::{
    assert_stage, grid_of_stage, rect_json, rect_of, FrameRecord,
};
use ratatui::{backend::TestBackend, buffer::Buffer, layout::Rect, Terminal};
use serde_json::{json, Value};

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
    let record = FrameRecord::framed(canvas, finished, &alone);
    json!({"framed": true, "canvas": rect_json(canvas),
        "overlay": record.overlay.map(rect_json)})
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
    let region = rect_of(&e["rect"]);
    let record = if info["framed"] == json!(true) {
        FrameRecord {
            framed: true,
            canvas: rect_of(&info["canvas"]),
            overlay: (!info["overlay"].is_null()).then(|| rect_of(&info["overlay"])),
        }
    } else {
        FrameRecord::unframed(region)
    };
    let bodies = (
        unframed_body_area(record.canvas),
        project_body_area(record.canvas),
    );
    assert_stage(
        path,
        &grid_of_stage(&a),
        &grid_of_stage(&e),
        region,
        &record,
        bodies,
    );
}
