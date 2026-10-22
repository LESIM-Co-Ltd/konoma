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

// The demo plugin (`demo` is on for this crate's tests through its dev-dependency on itself).
use konoma_plugin_ratatui::demo as clock;

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
fn symbols_a_canvas_cell_may_not_hold_become_blanks_and_the_canvas_passes_check() {
    let mut b = buf(8, 1);
    // Several graphemes, a cluster over 32 bytes, a three-column grapheme, ESC and C1.
    b[(0, 0)].set_symbol("ab");
    let long = format!("e{}", "\u{0301}".repeat(20));
    b[(1, 0)].set_symbol(&long);
    b[(2, 0)].set_symbol("\u{0600}日");
    b[(4, 0)].set_symbol("\u{1b}[31m");
    b[(5, 0)].set_symbol("\u{9b}");
    b[(6, 0)].set_symbol("ok");
    b[(7, 0)].set_symbol("z");
    let c = canvas_from_buffer(&b);
    for x in 0..6 {
        assert_eq!(sym(&c, x, 0), " ", "cell {x}");
    }
    assert_eq!(sym(&c, 7, 0), "z");
    assert_eq!(c.check(), Ok(()));
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
    open_bytes(&clock::Clock::default(), "a.konoma-demo", b"My title\n").unwrap()
}

#[test]
fn the_example_declares_what_it_does() {
    use konoma_plugin::Plugin as _;
    let m = clock::Clock::default().manifest();
    assert!(m.shows.contains(&ViewKind::Canvas));
    assert_eq!(check_manifest(&clock::Clock::default()), vec![]);
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
fn the_example_view_does_not_advance_state() {
    // `view` runs on a resize or after a key too: asking twice at once must give the same canvas.
    let (mut s, _f) = open_clock();
    let req = ViewReq::new(30, 5);
    let a = view_canvas(&mut *s, &req).unwrap();
    let b = view_canvas(&mut *s, &req).unwrap();
    assert_eq!(CanvasText::of(&a), CanvasText::of(&b));
}

#[test]
fn the_example_gauge_follows_the_time_since_open() {
    use std::time::Duration;
    let req = ViewReq::new(30, 5);
    let label = |age: u64| {
        let clock = clock::Clock {
            age: Duration::from_secs(age),
        };
        let (mut s, _f) = open_bytes(&clock, "a.konoma-demo", b"T\n").unwrap();
        let t = CanvasText::of(&view_canvas(&mut *s, &req).unwrap());
        t.rows[1].clone()
    };
    assert!(label(0).contains("0 s"), "{}", label(0));
    assert!(label(7).contains("7 s"), "{}", label(7));
    assert_ne!(label(3), label(4));
}

#[test]
fn the_example_ignores_ctrl_and_alt_keys_in_the_line_editor() {
    let (mut s, _f) = open_clock();
    let req = ViewReq::new(34, 6);
    s.act("edit", &ActCx::bare()).unwrap();
    let ctrl_c = Key::new(KeyCode::Char('c')).with_mods(KeyMods::CTRL);
    let alt_x = Key::new(KeyCode::Char('x')).with_mods(KeyMods::ALT);
    let out = send_keys(&mut *s, &[ctrl_c, alt_x, Key::char('a')])
        .outcomes()
        .unwrap();
    assert_eq!(
        out,
        vec![Outcome::Unhandled, Outcome::Unhandled, Outcome::Redraw]
    );
    let t = CanvasText::of(&view_canvas(&mut *s, &req).unwrap());
    assert!(t.rows.iter().any(|r| r.contains("> a█")), "{t}");
}

#[test]
fn the_example_picture_has_a_stable_id_and_is_unchanged_after_the_first_answer() {
    use konoma_plugin::session::TermCaps;
    let (mut s, _f) = open_clock();
    let req = ViewReq::new(30, 5);
    let cx = ViewCx::bare().with_term(TermCaps::new().pixels(true));
    let pic = |s: &mut dyn Session, req: &ViewReq| match s.view(req, &cx).unwrap() {
        View::Canvas(c) => {
            assert_eq!(c.check(), Ok(()));
            c.images
        }
        _ => unreachable!(),
    };
    let first = pic(&mut *s, &req);
    assert_eq!(first.len(), 1);
    assert!(
        first[0].image.is_some(),
        "the first answer carries the pixels"
    );
    let again = pic(&mut *s, &req);
    assert_eq!(again[0].id, first[0].id, "one stable id");
    assert!(again[0].image.is_none(), "later answers say unchanged");
    // A new area sends the pixels again (the picture moved).
    let resized = pic(&mut *s, &ViewReq::new(40, 6));
    assert!(resized[0].image.is_some());
    // No pixels, no picture.
    let (mut s2, _g) = open_clock();
    let none = match s2.view(&req, &ViewCx::bare()).unwrap() {
        View::Canvas(c) => c.images,
        _ => unreachable!(),
    };
    assert!(none.is_empty());
}

#[test]
fn the_example_picks_colors_by_the_terminal_depth() {
    use konoma_plugin::session::TermCaps;
    use konoma_plugin::style::Color;
    let req = ViewReq::new(30, 5);
    let border = |depth: ColorDepth| {
        let (mut s, _f) = open_clock();
        let cx = ViewCx::bare().with_term(TermCaps::new().colors(depth));
        match s.view(&req, &cx).unwrap() {
            View::Canvas(c) => c.cells[0].style.fg,
            _ => unreachable!(),
        }
    };
    assert_eq!(border(ColorDepth::Ansi16), Some(Color::Cyan));
    assert_eq!(border(ColorDepth::Ansi256), Some(Color::Indexed(80)));
    assert_eq!(
        border(ColorDepth::TrueColor),
        Some(Color::Rgb(80, 200, 220))
    );
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
    send_keys(&mut *s, &keys).outcomes().unwrap();
    let t = CanvasText::of(&view_canvas(&mut *s, &req).unwrap());
    assert!(t.rows.iter().any(|r| r.contains("> ab日本█")), "{t}");

    send_keys(&mut *s, &[Key::new(KeyCode::Backspace)])
        .outcomes()
        .unwrap();
    let t = CanvasText::of(&view_canvas(&mut *s, &req).unwrap());
    assert!(t.rows.iter().any(|r| r.contains("> ab日█")), "{t}");

    send_keys(&mut *s, &[Key::new(KeyCode::Enter)])
        .outcomes()
        .unwrap();
    let t = CanvasText::of(&view_canvas(&mut *s, &req).unwrap());
    assert!(t.rows.iter().any(|r| r.contains("- ab日")), "{t}");

    send_keys(&mut *s, &[Key::new(KeyCode::Esc)])
        .outcomes()
        .unwrap();
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
    .outcomes()
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

// ---- the demo's colour action and its 64 x 32 picture ----

#[test]
fn the_demo_arrow_actions_walk_the_gauge_colours_and_wrap() {
    use konoma_plugin::session::TermCaps;
    use konoma_plugin::style::Color;
    let req = ViewReq::new(30, 5);
    let (mut s, _f) = open_clock();
    let cx = ViewCx::bare().with_term(TermCaps::new().colors(ColorDepth::TrueColor));
    let gauge_fg = |s: &mut dyn Session| match s.view(&req, &cx).unwrap() {
        View::Canvas(c) => c.cells.iter().filter_map(|cell| cell.style.fg).find(|f| {
            matches!(
                f,
                Color::Rgb(90, 200, 120)
                    | Color::Rgb(210, 100, 200)
                    | Color::Rgb(230, 190, 60)
                    | Color::Rgb(90, 140, 240)
            )
        }),
        _ => unreachable!(),
    };
    let first = gauge_fg(&mut *s);
    assert_eq!(first, Some(Color::Rgb(90, 200, 120)));
    s.act("colour_next", &ActCx::bare()).unwrap();
    assert_eq!(gauge_fg(&mut *s), Some(Color::Rgb(210, 100, 200)));
    s.act("colour_prev", &ActCx::bare()).unwrap();
    s.act("colour_prev", &ActCx::bare()).unwrap();
    assert_eq!(
        gauge_fg(&mut *s),
        Some(Color::Rgb(90, 140, 240)),
        "wraps backwards"
    );
    for _ in 0..1 {
        s.act("colour_next", &ActCx::bare()).unwrap();
    }
    assert_eq!(gauge_fg(&mut *s), Some(first.unwrap()), "wraps forwards");
}

#[test]
fn the_demo_arrows_are_declared_on_left_and_right_and_the_picture_is_64_by_32() {
    use konoma_plugin::session::TermCaps;
    use konoma_plugin::Plugin as _;
    let m = clock::Clock::default().manifest();
    let key = |id: &str| m.actions.iter().find(|a| a.id == id).unwrap().key.clone();
    assert_eq!(key("colour_next").as_deref(), Some("Right"));
    assert_eq!(key("colour_prev").as_deref(), Some("Left"));
    let (mut s, _f) = open_clock();
    let cx = ViewCx::bare().with_term(TermCaps::new().pixels(true));
    for (cols, rows) in [(24, 5), (30, 5), (80, 24)] {
        let View::Canvas(c) = s.view(&ViewReq::new(cols, rows), &cx).unwrap() else {
            unreachable!()
        };
        assert_eq!(c.check(), Ok(()), "{cols}x{rows}");
        match &c.images[0].image {
            Some(konoma_plugin::view::ImageData::Rgba {
                width,
                height,
                pixels,
            }) => {
                assert_eq!((*width, *height), (64, 32));
                assert_eq!(pixels.len(), 64 * 32 * 4);
                assert_eq!(&pixels[..4], &[255, 255, 255, 255], "the border is white");
            }
            other => panic!("the first answer for {cols}x{rows} carries pixels: {other:?}"),
        }
    }
    // Too small for the picture: none, and still a valid canvas.
    let View::Canvas(c) = s.view(&ViewReq::new(23, 5), &cx).unwrap() else {
        unreachable!()
    };
    assert!(c.images.is_empty());
    let View::Canvas(c) = s.view(&ViewReq::new(30, 4), &cx).unwrap() else {
        unreachable!()
    };
    assert!(c.images.is_empty());
}

// ---- the demo explains itself, in both languages ----

fn demo_rows(lang: &str, cols: u16, rows: u16, keys: &[Key], edit: bool) -> CanvasText {
    use konoma_plugin::testing::view_canvas_with;
    let (mut s, _f) = open_clock();
    if edit {
        s.act("edit", &ActCx::bare()).unwrap();
    }
    send_keys(&mut *s, keys).outcomes().unwrap();
    let cx = ViewCx::new(Settings::empty(), lang);
    CanvasText::of(&view_canvas_with(&mut *s, &ViewReq::new(cols, rows), &cx).unwrap())
}

fn any_row(t: &CanvasText, needle: &str) -> bool {
    t.rows.iter().any(|r| r.contains(needle))
}

#[test]
fn the_demo_hint_line_names_the_keys_and_c_only_with_notes() {
    let en = demo_rows("en", 70, 6, &[], false);
    assert!(
        any_row(&en, "i: type a note   \u{2190}/\u{2192}: gauge colour"),
        "{en}"
    );
    assert!(!any_row(&en, "c: clear notes"), "{en}");
    let ja = demo_rows("ja", 70, 6, &[], false);
    assert!(any_row(&ja, "i: メモを入力"), "{ja}");
    assert!(!any_row(&ja, "メモを消す"), "{ja}");

    let keys = [
        Key::new(KeyCode::Char('a')),
        Key::new(KeyCode::Enter),
        Key::new(KeyCode::Esc),
    ];
    let en = demo_rows("en", 70, 6, &keys, true);
    assert!(any_row(&en, "c: clear notes"), "{en}");
    let ja = demo_rows("ja", 70, 6, &keys, true);
    assert!(any_row(&ja, "c: メモを消す"), "{ja}");
}

#[test]
fn the_demo_typing_banner_sits_above_the_prompt_in_both_languages() {
    let keys: Vec<Key> = "hello".chars().map(Key::char).collect();
    for (lang, banner) in [
        (
            "en",
            "Typing - type text, Enter adds it as a note, Esc leaves",
        ),
        ("ja", "入力中 — 文字を打って Enter で追加 / Esc で抜ける"),
    ] {
        let t = demo_rows(lang, 70, 6, &keys, true);
        let b = t.rows.iter().position(|r| r.contains(banner)).unwrap_or(99);
        let p = t
            .rows
            .iter()
            .position(|r| r.contains("> hello█"))
            .unwrap_or(98);
        assert_eq!(b + 1, p, "{lang}: banner directly above the prompt\n{t}");
        // No hint while typing.
        assert!(
            !any_row(&t, "gauge colour") && !any_row(&t, "ゲージ"),
            "{t}"
        );
    }
}

#[test]
fn the_demo_prompt_line_is_highlighted_across_its_width() {
    use konoma_plugin::testing::view_canvas_with;
    let (mut s, _f) = open_clock();
    s.act("edit", &ActCx::bare()).unwrap();
    let cx = ViewCx::bare();
    let c = view_canvas_with(&mut *s, &ViewReq::new(40, 6), &cx).unwrap();
    let t = CanvasText::of(&c);
    let y = t.rows.iter().position(|r| r.contains("> █")).unwrap();
    let hl = c.cells[y * 40..(y + 1) * 40]
        .iter()
        .filter(|cell| cell.style.bg.is_some())
        .count();
    assert!(hl >= 30, "the prompt row has a background: {hl} cells");
}

#[test]
fn the_demo_notes_have_a_heading_and_keep_the_newest_when_short() {
    let mut keys = Vec::new();
    for n in ["one", "two", "three", "four"] {
        keys.extend(n.chars().map(Key::char));
        keys.push(Key::new(KeyCode::Enter));
    }
    let en = demo_rows("en", 70, 11, &keys, true);
    assert!(any_row(&en, "Notes"), "{en}");
    assert!(any_row(&en, "- one") && any_row(&en, "- four"), "{en}");
    let ja = demo_rows("ja", 70, 11, &keys, true);
    assert!(any_row(&ja, "メモ"), "{ja}");
    // Short: 6 rows = border 2 + gauge 1 + 3 body rows = banner + prompt + one note row.
    let short = demo_rows("en", 70, 6, &keys, true);
    assert!(any_row(&short, "- four"), "{short}");
    assert!(!any_row(&short, "- one"), "{short}");
    // Idle, 2 notes rows (heading + one note): heading + the newest note.
    let mut leave = keys.clone();
    leave.push(Key::new(KeyCode::Esc));
    let idle = demo_rows("en", 70, 6, &leave, true);
    assert!(
        any_row(&idle, "Notes") && any_row(&idle, "- four"),
        "{idle}"
    );
    assert!(!any_row(&idle, "- three"), "{idle}");
}

#[test]
fn the_demo_text_stays_inside_tiny_areas() {
    let keys: Vec<Key> = "abc".chars().map(Key::char).collect();
    for (cols, rows) in [(3, 3), (10, 3), (10, 4), (12, 5), (30, 4)] {
        for edit in [false, true] {
            let t = demo_rows("ja", cols, rows, &keys, edit);
            assert_eq!(t.rows.len(), rows as usize);
        }
    }
}

#[test]
fn the_demo_screens_in_japanese_idle_typing_and_after_enter() {
    let typed: Vec<Key> = "hello".chars().map(Key::char).collect();
    let idle = demo_rows("ja", 56, 7, &[], false);
    let typing = demo_rows("ja", 56, 7, &typed, true);
    let mut sent = typed.clone();
    sent.push(Key::new(KeyCode::Enter));
    let after = demo_rows("ja", 56, 7, &sent, true);
    println!("idle:\n{idle}\ntyping:\n{typing}\nafter Enter:\n{after}");
    assert!(any_row(&idle, "i: メモを入力"));
    assert!(any_row(&typing, "> hello█") && any_row(&typing, "入力中"));
    assert!(any_row(&after, "- hello") && any_row(&after, "メモ"));
}

#[test]
fn the_demo_action_labels_do_not_collide_with_the_hosts_edit_key() {
    use konoma_plugin::Plugin as _;
    let m = clock::Clock::default().manifest();
    let label = |id: &str, lang: &str| {
        m.actions
            .iter()
            .find(|a| a.id == id)
            .unwrap()
            .label
            .get(lang)
            .to_string()
    };
    assert_eq!(
        (label("edit", "en"), label("edit", "ja")),
        ("note".into(), "メモ".into())
    );
    assert_eq!(label("clear", "en"), "clear notes");
    assert_eq!(label("clear", "ja"), "メモを消す");
    assert_eq!(
        (label("colour_next", "en"), label("colour_next", "ja")),
        ("colour".into(), "色".into())
    );
}

#[test]
fn the_kitty_placeholder_becomes_a_blank_and_the_canvas_passes_check() {
    let mut b = buf(3, 1);
    b[(0, 0)].set_symbol("\u{10EEEE}");
    b[(1, 0)].set_symbol("\u{10EEEE}\u{0305}");
    b[(2, 0)].set_symbol("z");
    let c = canvas_from_buffer(&b);
    assert_eq!(sym(&c, 0, 0), " ");
    assert_eq!(sym(&c, 1, 0), " ");
    assert_eq!(sym(&c, 2, 0), "z");
    assert!(c.check().is_ok());
}
