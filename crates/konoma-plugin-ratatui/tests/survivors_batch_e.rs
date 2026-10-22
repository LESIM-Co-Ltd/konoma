//! Tests added after the stage-2.5 mutation audit (axis 4), batch E: a cell the adapter blanks
//! keeps the style the plugin gave it (a background behind an empty or zero-width symbol, behind
//! half of a wide character at the right edge).

use konoma_plugin::style::{Color as PColor, Modifier as PModifier};
use konoma_plugin::view::{CanvasCell, CanvasView};
use konoma_plugin_ratatui::canvas_from_buffer;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color as RColor, Modifier as RModifier, Style as RStyle};

fn buf(cols: u16, rows: u16) -> Buffer {
    Buffer::empty(Rect::new(0, 0, cols, rows))
}

fn styled() -> RStyle {
    RStyle::new()
        .fg(RColor::Red)
        .bg(RColor::Blue)
        .add_modifier(RModifier::BOLD)
}

/// The cell is a blank (a space, not a continuation) with exactly the style `styled()` gives.
#[track_caller]
fn assert_styled_blank(c: &CanvasView, x: u16, y: u16) {
    let cell = c.cell(x, y).unwrap();
    assert_eq!(cell.symbol, " ", "cell ({x},{y}) is a blank");
    assert!(!cell.is_continuation());
    assert_eq!(cell.style.fg, Some(PColor::Red), "fg of ({x},{y})");
    assert_eq!(cell.style.bg, Some(PColor::Blue), "bg of ({x},{y})");
    assert!(
        cell.style.add.contains(PModifier::BOLD),
        "bold of ({x},{y})"
    );
}

#[test]
fn an_empty_symbol_becomes_a_blank_that_keeps_its_style() {
    let mut b = buf(3, 1);
    b[(1, 0)].set_symbol("").set_style(styled());
    let c = canvas_from_buffer(&b);
    assert_styled_blank(&c, 1, 0);
    // The cells around it are untouched.
    assert_eq!(*c.cell(0, 0).unwrap(), CanvasCell::default());
    assert_eq!(*c.cell(2, 0).unwrap(), CanvasCell::default());
}

#[test]
fn a_zero_width_symbol_becomes_a_blank_that_keeps_its_style() {
    let mut b = buf(3, 2);
    // A zero-width space and a lone combining mark, each with a style, on two rows.
    b[(0, 0)].set_symbol("\u{200B}").set_style(styled());
    b[(2, 1)].set_symbol("\u{0301}").set_style(styled());
    let c = canvas_from_buffer(&b);
    assert_styled_blank(&c, 0, 0);
    assert_styled_blank(&c, 2, 1);
    assert_eq!(c.check(), Ok(()));
}

#[test]
fn half_a_wide_character_at_the_right_edge_becomes_a_blank_that_keeps_its_style() {
    let mut b = buf(3, 2);
    // A wide character in the last column of each row: only half would fit.
    b[(2, 0)].set_symbol("日").set_style(styled());
    b[(2, 1)].set_symbol("😀").set_style(styled());
    let c = canvas_from_buffer(&b);
    assert_styled_blank(&c, 2, 0);
    assert_styled_blank(&c, 2, 1);
    assert_eq!(c.check(), Ok(()));
    // A wide character that does fit keeps its symbol and gives its style to the continuation too.
    let mut b = buf(3, 1);
    b[(0, 0)].set_symbol("日").set_style(styled());
    let c = canvas_from_buffer(&b);
    assert_eq!(c.cell(0, 0).unwrap().symbol, "日");
    assert!(c.cell(1, 0).unwrap().is_continuation());
    assert_eq!(c.cell(1, 0).unwrap().style.bg, Some(PColor::Blue));
}
