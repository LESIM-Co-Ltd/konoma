//! `konoma_plugin::symbol_width` must equal the width ratatui uses when it draws a cell, because
//! the host draws a canvas through ratatui.

use konoma_plugin::style::Style as PStyle;
use konoma_plugin::{symbol_width, CanvasCell, CanvasView};
use konoma_plugin_ratatui::{canvas_from_buffer, draw_canvas};
use ratatui::buffer::{Buffer, CellWidth};
use ratatui::layout::Rect;
use ratatui::style::Style as RStyle;

fn ratatui_width(s: &str) -> usize {
    s.cell_width() as usize
}

#[test]
fn every_bmp_code_point_has_ratatuis_width() {
    let mut bad = Vec::new();
    for cp in 0x20u32..=0xFFFF {
        let Some(c) = char::from_u32(cp) else {
            continue;
        };
        if c.is_control() {
            // Never a symbol (a canvas refuses it, ratatui filters it before it counts).
            continue;
        }
        let s = c.to_string();
        if symbol_width(&s) != ratatui_width(&s) {
            bad.push(format!(
                "U+{cp:04X} ours {} ratatui {}",
                symbol_width(&s),
                ratatui_width(&s)
            ));
        }
    }
    assert!(
        bad.is_empty(),
        "{} differ: {:?}",
        bad.len(),
        &bad[..bad.len().min(20)]
    );
}

#[test]
fn sequences_have_ratatuis_width() {
    let cases = [
        "a",
        " ",
        "あ",
        "漢",
        "ｱ",
        "ｶﾞ",
        "ﾊﾟ",
        "ﾃﾞｰﾀ",
        "ﾞ",
        "ﾟ",
        "ｶﾞﾞ",
        "e\u{301}",
        "\u{3099}",
        "か\u{3099}",
        "\u{1F600}",
        "\u{2764}\u{FE0F}",
        "\u{2764}",
        "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}",
        "\u{1F1EF}\u{1F1F5}",
        "\u{1F44D}\u{1F3FD}",
        "1\u{FE0F}\u{20E3}",
        "\u{200B}",
        "\u{200D}",
        "\u{FE0F}",
        "\u{0E01}\u{0E33}",
        "각",
        "\u{1100}\u{1161}\u{11A8}",
    ];
    for s in cases {
        assert_eq!(symbol_width(s), ratatui_width(s), "{s:?}");
    }
}

#[test]
fn halfwidth_sound_marks_are_a_column_each() {
    assert_eq!(symbol_width("ｶﾞ"), 2);
    assert_eq!(symbol_width("ﾞ"), 1);
    assert_eq!(symbol_width("ﾃﾞｰﾀ"), 4);
}

#[test]
fn a_ratatui_paragraph_with_voiced_halfwidth_kana_converts_to_a_valid_canvas() {
    let area = Rect::new(0, 0, 12, 1);
    let mut b = Buffer::empty(area);
    b.set_string(0, 0, "ﾃﾞｰﾀ", RStyle::new());
    let c = canvas_from_buffer(&b);
    assert_eq!(
        c.check(),
        Ok(()),
        "{:?}",
        c.cells.iter().map(|x| &x.symbol).collect::<Vec<_>>()
    );
    // And back: the canvas draws to the same buffer ratatui made.
    let back = draw_canvas(&konoma_plugin::ViewReq::new(12, 1), |bf| {
        bf.set_string(0, 0, "ﾃﾞｰﾀ", RStyle::new());
    });
    assert_eq!(back.check(), Ok(()));
    assert_eq!(c.cells, back.cells);
}

#[test]
fn forced_width_does_not_change_the_canvas() {
    use ratatui::buffer::CellDiffOption;
    use std::num::NonZeroU16;
    let mut b = Buffer::empty(Rect::new(0, 0, 4, 1));
    b.set_string(0, 0, "ab", RStyle::new());
    b[(0, 0)].diff_option = CellDiffOption::ForcedWidth(NonZeroU16::new(2).unwrap());
    let c = canvas_from_buffer(&b);
    assert_eq!(c.check(), Ok(()));
    assert_eq!(c.cell(0, 0).unwrap().symbol, "a");
    assert_eq!(c.cell(1, 0).unwrap().symbol, "b");
}

#[test]
fn set_string_agrees_with_ratatui_about_voiced_kana() {
    let mut c = CanvasView::new(10, 1);
    let used = c.set_string(0, 0, "ﾃﾞｰﾀ", PStyle::default());
    assert_eq!(used as usize, symbol_width("ﾃﾞｰﾀ"));
    assert_eq!(c.check(), Ok(()));
    // Draw it the way the host does (through ratatui) and read the width ratatui assigns to
    // each non-continuation cell: it matches the cells the plugin wrote.
    let mut covered = 0usize;
    for x in 0..10 {
        let cell = c.cell(x, 0).unwrap();
        if cell.is_continuation() {
            continue;
        }
        if cell.symbol != " " {
            covered += ratatui_width(&cell.symbol);
        }
    }
    assert_eq!(covered, used as usize);
    let cell = CanvasCell::new("ﾞ", PStyle::default());
    assert!(CanvasCell::is_plain_symbol(&cell.symbol));
}
