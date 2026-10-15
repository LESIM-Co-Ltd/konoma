//! Tests added after the stage-2.5 mutation audit (axis 4), batch E: `KeyMods::contains` with a
//! multi-bit mask, the columns `set_string` reports for a row below the canvas, and the keys a
//! plugin without `shows` cannot take because a canvas reserves them.

use konoma_plugin::key::KeyMods;
use konoma_plugin::keys::{reserved, validate, KeyIssueKind};
use konoma_plugin::prelude::*;
use konoma_plugin::view::CanvasView;
use konoma_plugin::Style;

// ---- KeyMods::contains (m045) ------------------------------------------------------------------

#[test]
fn contains_needs_every_bit_of_the_mask_not_any_of_them() {
    let both = KeyMods::CTRL | KeyMods::ALT;
    assert!(both.contains(KeyMods::CTRL | KeyMods::ALT));
    assert!(both.contains(KeyMods::CTRL));
    assert!(both.contains(KeyMods::ALT));
    // One bit short of the mask: not contained, although one bit of the mask is set.
    assert!(!KeyMods::CTRL.contains(KeyMods::CTRL | KeyMods::ALT));
    assert!(!KeyMods::ALT.contains(KeyMods::CTRL | KeyMods::ALT));
    assert!(!KeyMods::NONE.contains(KeyMods::CTRL | KeyMods::ALT));
    // A three-bit mask with two bits held.
    let all = KeyMods::CTRL | KeyMods::ALT | KeyMods::SHIFT;
    assert!(!both.contains(all));
    assert!(all.contains(both));
    // The empty mask is in everything.
    assert!(KeyMods::NONE.contains(KeyMods::NONE));
    assert!(KeyMods::CTRL.contains(KeyMods::NONE));
}

// ---- CanvasView::set_string below the canvas (m013) ---------------------------------------------

#[test]
fn set_string_on_a_row_below_the_canvas_takes_no_columns() {
    let mut c = CanvasView::new(8, 3);
    // The last row takes the text; the row just below and any row further down take none.
    assert_eq!(c.set_string(0, 2, "abc", Style::new()), 3);
    assert_eq!(
        c.set_string(0, 3, "abc", Style::new()),
        0,
        "the row just below"
    );
    assert_eq!(c.set_string(0, 4, "abc", Style::new()), 0);
    assert_eq!(c.set_string(0, u16::MAX, "abc", Style::new()), 0);
    assert_eq!(c.check(), Ok(()));
    // Nothing leaked into the canvas.
    assert_eq!(c.cells.iter().filter(|x| x.symbol != " ").count(), 3);
}

#[test]
fn set_string_on_a_canvas_without_rows_takes_no_columns() {
    let mut c = CanvasView::new(8, 0);
    assert_eq!(c.set_string(0, 0, "abc", Style::new()), 0);
}

// ---- the canvas keys are reserved for a plugin that says nothing about `shows` (m059) -----------

/// A manifest with no `shows` whose only action sits on `key`.
fn manifest_with_key(key: &str) -> Manifest {
    Manifest::new("k", "0.1.0")
        .bind(Bind::files(["*.k"]))
        .action(Act::new("go").key(key).label(Tr::new("go")))
}

#[test]
fn a_plugin_without_shows_cannot_take_a_key_a_canvas_reserves() {
    let canvas_keys = reserved("canvas");
    assert!(!canvas_keys.is_empty());
    for (key, _) in canvas_keys {
        let issues = validate(&manifest_with_key(&key.to_string()));
        assert!(
            issues
                .iter()
                .any(|i| matches!(i.kind, KeyIssueKind::Reserved { .. })),
            "`{key}` is reserved in a canvas but a plugin that shows nothing in particular may take it: {issues:?}"
        );
    }
}

/// `VIEW_CONTEXTS` (the contexts a manifest without `shows` is checked against) must cover every
/// display type's own reserved keys, `canvas` included. Today the canvas set is inside the union of
/// the other display types, so dropping `canvas` from the list changes nothing a test can see
/// (an equivalent change); this guard fails the day a canvas reserves a key the others do not.
#[test]
fn every_display_types_reserved_keys_are_reserved_for_a_plugin_without_shows() {
    for context in ["text", "document", "table", "image", "diff", "canvas"] {
        for (key, _) in reserved(context) {
            let issues = validate(&manifest_with_key(&key.to_string()));
            assert!(
                issues
                    .iter()
                    .any(|i| matches!(i.kind, KeyIssueKind::Reserved { .. })),
                "`{key}` ({context}) is free for a plugin without shows: {issues:?}"
            );
        }
    }
}
