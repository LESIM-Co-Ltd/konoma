//! Tests added after round 2 of the stage-2.5 mutation audit: a joiner that failed to join, and
//! `KeyMods |=` with a bit that is already set.

use konoma_plugin::key::KeyMods;
use konoma_plugin::view::CanvasView;
use konoma_plugin::Style;

fn sym(c: &CanvasView, x: u16) -> String {
    c.cell(x, 0).unwrap().symbol.clone()
}

/// A zero-width joiner with nothing to join (at the start) is dropped, and the character after it
/// is an ordinary character, not another failed join.
#[test]
fn a_joiner_that_joined_nothing_does_not_swallow_the_next_character() {
    let mut c = CanvasView::new(10, 1);
    let n = c.set_string(0, 0, "\u{200D}ab", Style::new());
    assert_eq!(n, 2, "the joiner took no column");
    assert_eq!(sym(&c, 0), "a");
    assert_eq!(sym(&c, 1), "b");
}

/// The same after a joiner the cell before it cannot take (the cell holds as many marks as it may):
/// the character after the failed joiner still lands in its own cell.
#[test]
fn a_joiner_refused_by_a_full_cell_does_not_swallow_the_next_character() {
    let mut c = CanvasView::new(10, 1);
    let marks = "\u{301}".repeat(40);
    let n = c.set_string(0, 0, &format!("a{marks}\u{200D}b"), Style::new());
    assert_eq!(n, 2, "a and b, one column each");
    assert_eq!(sym(&c, 1), "b");
    assert!(sym(&c, 0).starts_with('a'));
    assert!(
        !sym(&c, 0).contains('\u{200D}'),
        "the full cell did not take the joiner"
    );
}

/// `|=` sets a bit; setting one that is set stays set (it is not toggled).
#[test]
fn or_assign_on_a_set_bit_keeps_it_set() {
    let mut m = KeyMods::CTRL;
    m |= KeyMods::CTRL;
    assert_eq!(m, KeyMods::CTRL);
    assert!(m.contains(KeyMods::CTRL));
    m |= KeyMods::ALT;
    m |= KeyMods::CTRL;
    assert_eq!(m, KeyMods::CTRL | KeyMods::ALT);
    let mut n = KeyMods::NONE;
    n |= KeyMods::NONE;
    assert!(n.is_empty());
}
