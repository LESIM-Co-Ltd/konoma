//! The standalone app's side of the frozen-fixture comparison for framed project content. The
//! fixtures were captured before the list frame existed, so each frozen body cell is compared at
//! its new place inside the frame and the frame and top edge against the rule; the mapping is
//! `konoma_test_support::frozen_frame`, the same code the host's tests use.
use konoma_plugin::host::project::draw::{project_body_area, unframed_body_area};
use konoma_test_support::frozen_frame::{
    assert_stage, grid_of_buffer, grid_of_rows, grid_of_stage, FrameRecord,
};
use ratatui::{buffer::Buffer, layout::Rect};
use serde_json::Value;

/// How the last project frame was drawn: framed content (with the overlay found as what differs
/// from the canvas drawn alone) or not.
fn record_of(finished: &Buffer) -> (FrameRecord, (Rect, Rect)) {
    let canvas = konoma_plugin::app::project::test_canvas();
    assert_eq!(
        canvas.alone.area, finished.area,
        "the recorded canvas belongs to the finished frame"
    );
    let record = if canvas.framed {
        FrameRecord::framed(canvas.area, finished, &canvas.alone)
    } else {
        FrameRecord::unframed(canvas.area)
    };
    let bodies = (
        unframed_body_area(canvas.area),
        project_body_area(canvas.area),
    );
    (record, bodies)
}

/// The cells of `region` of the finished frame against a frozen stage (`rect`, `count`, `runs`).
#[track_caller]
pub fn assert_frozen_stage(path: &str, finished: &Buffer, region: Rect, expected: &Value) {
    assert_eq!(
        expected["rect"],
        serde_json::json!([region.x, region.y, region.width, region.height]),
        "{path}: the frozen region"
    );
    let old = grid_of_stage(expected);
    assert_eq!(expected["count"], old.len(), "{path}: the frozen count");
    let (record, bodies) = record_of(finished);
    assert_stage(
        path,
        &grid_of_buffer(finished, region),
        &old,
        region,
        &record,
        bodies,
    );
    assert!(
        old.iter()
            .any(|c| !c[0].as_str().unwrap().trim().is_empty()),
        "{path}: the frozen stage shows something"
    );
}

/// The whole finished frame against a frozen frame kept as text rows and style runs.
#[track_caller]
pub fn assert_frozen_rows(path: &str, finished: &Buffer, rows: &Value, styles: &Value) {
    let region = finished.area;
    let old = grid_of_rows(rows, styles, region);
    let (record, bodies) = record_of(finished);
    assert_stage(
        path,
        &grid_of_buffer(finished, region),
        &old,
        region,
        &record,
        bodies,
    );
}
