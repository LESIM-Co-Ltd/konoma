//! The Buffer -> CanvasView conversion and the example plugin run through the testing helpers.

use konoma_plugin::key::{Key, KeyCode};
use konoma_plugin::prelude::*;
use konoma_plugin::style::{Color as PColor, Modifier as PModifier};
use konoma_plugin::testing::{check_manifest, open_bytes, send_keys, view_canvas, CanvasText};
use konoma_plugin_ratatui::{
    canvas_buffer, canvas_from_buffer, color_of, draw_canvas, modifier_of,
};
use ratatui::buffer::{Buffer, Cell, CellDiffOption};
use ratatui::layout::Rect;
use ratatui::style::{Color as RColor, Modifier as RModifier, Style as RStyle};
use ratatui::widgets::{Block, Paragraph, Widget};

#[allow(dead_code)]
#[path = "../examples/clock.rs"]
mod clock;

fn buf(cols: u16, rows: u16) -> Buffer {
    Buffer::empty(Rect::new(0, 0, cols, rows))
}

fn sym(c: &CanvasView, x: u16, y: u16) -> String {
    c.cell(x, y).unwrap().symbol.clone()
}

#[test]
fn empty_buffer_is_a_blank_canvas() {
    let c = canvas_from_buffer(&buf(4, 3));
    assert_eq!((c.cols, c.rows), (4, 3));
    assert!(c.cells.iter().all(|x| *x == CanvasCell::default()));
    assert_eq!(c.check(), Ok(()));
    let z = canvas_from_buffer(&buf(0, 0));
    assert!(z.cells.is_empty() && z.check().is_ok());
}

#[test]
fn ascii_text_and_styles_are_kept() {
    let mut b = buf(6, 2);
    b.set_string(1, 1, "abc", RStyle::new().fg(RColor::Red).bg(RColor::Blue));
    let c = canvas_from_buffer(&b);
    assert_eq!(CanvasText::of(&c).rows, vec!["", " abc"]);
    let s = c.cell(2, 1).unwrap().style;
    assert_eq!((s.fg, s.bg), (Some(PColor::Red), Some(PColor::Blue)));
    assert_eq!(c.cell(0, 1).unwrap().style, Style::default());
}

#[test]
fn an_area_not_at_the_origin_is_read_from_its_own_corner() {
    let mut b = Buffer::empty(Rect::new(5, 7, 3, 2));
    b.set_string(5, 7, "xy", RStyle::new());
    b.set_string(6, 8, "z", RStyle::new());
    let c = canvas_from_buffer(&b);
    assert_eq!((c.cols, c.rows), (3, 2));
    assert_eq!(CanvasText::of(&c).rows, vec!["xy", " z"]);
}

#[test]
fn cjk_gets_a_continuation_cell() {
    let mut b = buf(6, 1);
    b.set_string(0, 0, "日本a", RStyle::new().fg(RColor::Green));
    let c = canvas_from_buffer(&b);
    assert_eq!(sym(&c, 0, 0), "日");
    assert!(c.cell(1, 0).unwrap().is_continuation());
    assert_eq!(sym(&c, 2, 0), "本");
    assert!(c.cell(3, 0).unwrap().is_continuation());
    assert_eq!(sym(&c, 4, 0), "a");
    assert_eq!(sym(&c, 5, 0), " ");
    // The continuation carries the wide cell's style, as `CanvasView::set_string` does.
    assert_eq!(c.cell(1, 0).unwrap().style.fg, Some(PColor::Green));
    assert_eq!(CanvasText::of(&c).rows, vec!["日本a"]);
    assert_eq!(c.check(), Ok(()));
}

#[test]
fn the_cell_after_a_wide_char_is_a_continuation_whatever_ratatui_holds_there() {
    // ratatui leaves a space after a wide char; other producers leave "" or a skipped cell.
    for fill in [" ", "", "x"] {
        let mut b = buf(3, 1);
        b.set_string(0, 0, "日", RStyle::new());
        b[(1, 0)].set_symbol(fill);
        let c = canvas_from_buffer(&b);
        assert_eq!(sym(&c, 0, 0), "日");
        assert!(c.cell(1, 0).unwrap().is_continuation(), "fill {fill:?}");
        assert_eq!(sym(&c, 2, 0), " ");
    }
    let mut b = buf(3, 1);
    b.set_string(0, 0, "日", RStyle::new());
    b[(1, 0)].set_diff_option(CellDiffOption::Skip);
    assert!(canvas_from_buffer(&b).cell(1, 0).unwrap().is_continuation());
}

#[test]
fn a_wide_char_in_the_last_column_is_never_half_written() {
    let mut b = buf(3, 1);
    // Put the symbol by hand: set_string would clip it.
    b[(2, 0)].set_symbol("日");
    let c = canvas_from_buffer(&b);
    assert_eq!(sym(&c, 2, 0), " ", "no half of a wide char");
    assert_eq!(c.check(), Ok(()));
    // The same wide char one column to the left fits.
    let mut b = buf(3, 1);
    b[(1, 0)].set_symbol("日");
    let c = canvas_from_buffer(&b);
    assert_eq!(sym(&c, 1, 0), "日");
    assert!(c.cell(2, 0).unwrap().is_continuation());
}

#[test]
fn every_wide_char_is_followed_by_exactly_one_continuation() {
    // The invariant the host relies on, over a mixed row.
    let mut b = buf(20, 1);
    b.set_string(0, 0, "a日b😀c本語d", RStyle::new());
    let c = canvas_from_buffer(&b);
    use ratatui::buffer::CellWidth;
    for x in 0..c.cols {
        let cell = c.cell(x, 0).unwrap();
        if cell.is_continuation() {
            assert!(x > 0 && c.cell(x - 1, 0).unwrap().symbol.cell_width() == 2);
        } else if cell.symbol.cell_width() == 2 {
            assert!(c.cell(x + 1, 0).unwrap().is_continuation());
        }
    }
}

#[test]
fn emoji_is_two_wide() {
    let mut b = buf(5, 1);
    b.set_string(0, 0, "😀b", RStyle::new());
    let c = canvas_from_buffer(&b);
    assert_eq!(sym(&c, 0, 0), "😀");
    assert!(c.cell(1, 0).unwrap().is_continuation());
    assert_eq!(sym(&c, 2, 0), "b");
}

#[test]
fn zero_width_and_control_symbols_become_blanks() {
    let mut b = buf(4, 1);
    b[(0, 0)].set_symbol("");
    b[(1, 0)].set_symbol("\n");
    b[(2, 0)].set_symbol("\u{200B}");
    b[(3, 0)].set_symbol("z");
    let c = canvas_from_buffer(&b);
    assert_eq!(sym(&c, 0, 0), " ");
    assert_eq!(sym(&c, 1, 0), " ");
    assert_eq!(sym(&c, 2, 0), " ");
    assert_eq!(sym(&c, 3, 0), "z");
    assert!(c.cells.iter().all(|x| !x.is_continuation()));
}

#[test]
fn skipped_cells_become_default_cells() {
    let mut b = buf(3, 1);
    b.set_string(0, 0, "abc", RStyle::new().fg(RColor::Red).bg(RColor::Blue));
    b[(1, 0)].set_diff_option(CellDiffOption::Skip);
    #[allow(deprecated)]
    {
        b[(2, 0)].skip = true;
    }
    let c = canvas_from_buffer(&b);
    assert_eq!(sym(&c, 0, 0), "a");
    assert_eq!(*c.cell(1, 0).unwrap(), CanvasCell::default());
    assert_eq!(*c.cell(2, 0).unwrap(), CanvasCell::default());
}

#[test]
fn every_color_kind_maps_one_to_one() {
    let pairs = [
        (RColor::Black, PColor::Black),
        (RColor::Red, PColor::Red),
        (RColor::Green, PColor::Green),
        (RColor::Yellow, PColor::Yellow),
        (RColor::Blue, PColor::Blue),
        (RColor::Magenta, PColor::Magenta),
        (RColor::Cyan, PColor::Cyan),
        (RColor::Gray, PColor::Gray),
        (RColor::DarkGray, PColor::DarkGray),
        (RColor::LightRed, PColor::LightRed),
        (RColor::LightGreen, PColor::LightGreen),
        (RColor::LightYellow, PColor::LightYellow),
        (RColor::LightBlue, PColor::LightBlue),
        (RColor::LightMagenta, PColor::LightMagenta),
        (RColor::LightCyan, PColor::LightCyan),
        (RColor::White, PColor::White),
        (RColor::Indexed(0), PColor::Indexed(0)),
        (RColor::Indexed(214), PColor::Indexed(214)),
        (RColor::Rgb(1, 2, 3), PColor::Rgb(1, 2, 3)),
        (RColor::Rgb(255, 255, 255), PColor::Rgb(255, 255, 255)),
    ];
    for (r, p) in pairs {
        assert_eq!(color_of(r), Some(p), "{r:?}");
        let mut b = buf(1, 1);
        b[(0, 0)].set_symbol("x").set_fg(r).set_bg(r);
        let s = canvas_from_buffer(&b).cells[0].style;
        assert_eq!((s.fg, s.bg), (Some(p), Some(p)), "{r:?}");
    }
    // Reset is "no color".
    assert_eq!(color_of(RColor::Reset), None);
    let mut b = buf(1, 1);
    b[(0, 0)].set_symbol("x");
    let s = canvas_from_buffer(&b).cells[0].style;
    assert_eq!((s.fg, s.bg), (None, None));
}

#[test]
fn every_modifier_maps_to_its_own_bit() {
    let pairs = [
        (RModifier::BOLD, PModifier::BOLD),
        (RModifier::DIM, PModifier::DIM),
        (RModifier::ITALIC, PModifier::ITALIC),
        (RModifier::UNDERLINED, PModifier::UNDERLINED),
        (RModifier::SLOW_BLINK, PModifier::SLOW_BLINK),
        (RModifier::RAPID_BLINK, PModifier::RAPID_BLINK),
        (RModifier::REVERSED, PModifier::REVERSED),
        (RModifier::HIDDEN, PModifier::HIDDEN),
        (RModifier::CROSSED_OUT, PModifier::CROSSED_OUT),
    ];
    for (r, p) in pairs {
        assert_eq!(modifier_of(r), p, "{r:?}");
        // The numeric bits are the API's promise, which the host relies on.
        assert_eq!(p.bits(), r.bits(), "{r:?}");
        let mut b = buf(1, 1);
        b[(0, 0)].set_symbol("x").modifier = r;
        assert_eq!(canvas_from_buffer(&b).cells[0].style.add, p);
    }
    let all = pairs.iter().fold(RModifier::empty(), |a, (r, _)| a | *r);
    let all_p = pairs.iter().fold(PModifier::EMPTY, |a, (_, p)| a | *p);
    assert_eq!(modifier_of(all), all_p);
    assert_eq!(modifier_of(RModifier::empty()), PModifier::EMPTY);
    // The modifiers go only to `add`.
    let mut b = buf(1, 1);
    b[(0, 0)].set_symbol("x").modifier = all;
    assert_eq!(canvas_from_buffer(&b).cells[0].style.sub, PModifier::EMPTY);
}

#[test]
fn a_cell_built_from_the_default_is_the_default_cell() {
    let mut b = buf(1, 1);
    b[(0, 0)] = Cell::default();
    assert_eq!(canvas_from_buffer(&b).cells[0], CanvasCell::default());
}

#[test]
fn widgets_drawn_into_a_request_sized_buffer_make_a_valid_canvas() {
    let req = ViewReq::new(20, 5);
    assert_eq!(canvas_buffer(&req).area, Rect::new(0, 0, 20, 5));
    let c = draw_canvas(&req, |buf| {
        let block = Block::bordered().title("日本語 title");
        let inner = block.inner(buf.area);
        block.render(buf.area, buf);
        Paragraph::new("日本語のテキスト\nplain")
            .style(RStyle::new().add_modifier(RModifier::BOLD).fg(RColor::Cyan))
            .render(inner, buf);
    });
    assert_eq!(c.check(), Ok(()));
    assert_eq!((c.cols, c.rows), (20, 5));
    let t = CanvasText::of(&c);
    assert!(t.rows[0].starts_with("┌日本語 title"), "{t}");
    assert!(t.rows[1].starts_with("│日本語のテキスト"), "{t}");
    assert_eq!(t.rows[2], "│plain             │");
    // Wide chars written by the Paragraph have their continuation cells.
    assert_eq!(sym(&c, 1, 1), "日");
    assert!(c.cell(2, 1).unwrap().is_continuation());
    let s = c.cell(1, 1).unwrap().style;
    assert_eq!((s.fg, s.add), (Some(PColor::Cyan), PModifier::BOLD));
    // Plain data: no pictures and no timers unless the plugin adds them.
    assert!(c.images.is_empty() && c.redraw_after.is_none() && !c.text_input);
}

#[test]
fn a_zero_sized_request_draws_an_empty_valid_canvas() {
    let c = draw_canvas(&ViewReq::new(0, 0), |buf| {
        Block::bordered().render(buf.area, buf);
    });
    assert!(c.cells.is_empty());
    assert_eq!(c.check(), Ok(()));
}

// ---- the example plugin through the testing helpers ----

fn open_clock() -> (Box<dyn Session>, konoma_plugin::testing::Scratch) {
    open_bytes(&clock::Clock, "a.konoma-demo", b"My title\n").unwrap()
}

#[test]
fn the_example_declares_what_it_does() {
    use konoma_plugin::Plugin as _;
    let m = clock::Clock.manifest();
    assert!(m.shows.contains(&ViewKind::Canvas));
    assert_eq!(check_manifest(&clock::Clock), vec![]);
    assert!(m.actions.iter().any(|a| a.id == "edit"));
}

#[test]
fn the_example_returns_a_canvas_sized_to_the_request() {
    let (mut s, _f) = open_clock();
    for (cols, rows) in [(30, 6), (12, 4), (80, 24)] {
        let c = view_canvas(&mut *s, &ViewReq::new(cols, rows)).unwrap();
        assert_eq!((c.cols, c.rows), (cols, rows));
        assert_eq!(c.redraw_after, Some(std::time::Duration::from_secs(1)));
        assert!(!c.text_input);
    }
    let c = view_canvas(&mut *s, &ViewReq::new(30, 6)).unwrap();
    assert!(CanvasText::of(&c).rows[0].contains("My title"));
}

#[test]
fn the_example_gauge_moves_between_redraws() {
    let (mut s, _f) = open_clock();
    let req = ViewReq::new(30, 5);
    let a = view_canvas(&mut *s, &req).unwrap();
    let b = view_canvas(&mut *s, &req).unwrap();
    assert_ne!(CanvasText::of(&a), CanvasText::of(&b));
}

#[test]
fn keys_typed_in_input_mode_appear_and_esc_leaves() {
    let (mut s, _f) = open_clock();
    let req = ViewReq::new(34, 6);
    assert_eq!(s.act("edit", &ActCx::bare()).unwrap(), Outcome::Redraw);
    let c = view_canvas(&mut *s, &req).unwrap();
    assert!(c.text_input, "the edit action asks for text input");

    let mut keys: Vec<Key> = "ab".chars().map(Key::char).collect();
    keys.push(Key::paste("日本"));
    send_keys(&mut *s, &keys).unwrap();
    let t = CanvasText::of(&view_canvas(&mut *s, &req).unwrap());
    assert!(t.rows.iter().any(|r| r.contains("> ab日本_")), "{t}");

    send_keys(&mut *s, &[Key::new(KeyCode::Backspace)]).unwrap();
    let t = CanvasText::of(&view_canvas(&mut *s, &req).unwrap());
    assert!(t.rows.iter().any(|r| r.contains("> ab日_")), "{t}");

    send_keys(&mut *s, &[Key::new(KeyCode::Enter)]).unwrap();
    let t = CanvasText::of(&view_canvas(&mut *s, &req).unwrap());
    assert!(t.rows.iter().any(|r| r.contains("- ab日")), "{t}");

    send_keys(&mut *s, &[Key::new(KeyCode::Esc)]).unwrap();
    let c = view_canvas(&mut *s, &req).unwrap();
    assert!(!c.text_input, "Esc leaves the input state");
}

#[test]
fn a_declared_action_is_gated_by_enabled_and_clears_the_notes() {
    let (mut s, _f) = open_clock();
    let req = ViewReq::new(34, 6);
    assert!(!s.enabled("clear"));
    s.act("edit", &ActCx::bare()).unwrap();
    send_keys(
        &mut *s,
        &[
            Key::char('n'),
            Key::new(KeyCode::Enter),
            Key::new(KeyCode::Esc),
        ],
    )
    .unwrap();
    assert!(s.enabled("clear"));
    assert!(CanvasText::of(&view_canvas(&mut *s, &req).unwrap())
        .rows
        .iter()
        .any(|r| r.contains("- n")));
    assert_eq!(s.act("clear", &ActCx::bare()).unwrap(), Outcome::Redraw);
    assert!(!CanvasText::of(&view_canvas(&mut *s, &req).unwrap())
        .rows
        .iter()
        .any(|r| r.contains("- n")));
    assert_eq!(s.act("nope", &ActCx::bare()).unwrap(), Outcome::Unhandled);
}
