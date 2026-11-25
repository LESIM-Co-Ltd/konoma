//! `CanvasView::check`, the single "plain data is safe" contract, and the forward-compatible
//! construction of the API types (they are `#[non_exhaustive]`: an outside crate can only build
//! them through constructors and builders, as this file does).

use std::sync::Arc;
use std::time::Duration;

use konoma_plugin::key::{Key, KeyCode};
use konoma_plugin::prelude::*;
use konoma_plugin::session::{ColorDepth, TermCaps};
use konoma_plugin::view::{
    CanvasCell, CanvasImage, CanvasIssue, CanvasView, CellRect, ImageData, CANVAS_MAX_CELLS,
    CANVAS_MAX_FRAMES, CANVAS_MAX_PICTURES, CANVAS_MAX_PLACEMENTS, CANVAS_MAX_RGBA_SIDE,
    CANVAS_MAX_SIDE, CANVAS_MAX_SYMBOL_BYTES,
};

/// A canvas of `cols` x 1 whose cell `x` holds `symbol`.
fn with_symbol(cols: u16, x: u16, symbol: &str) -> CanvasView {
    let mut c = CanvasView::new(cols, 1);
    c.cell_mut(x, 0).unwrap().symbol = symbol.to_string();
    c
}

fn rgba(w: u32, h: u32, bytes: usize) -> ImageData {
    ImageData::Rgba {
        width: w,
        height: h,
        pixels: Arc::new(vec![0; bytes]),
    }
}

fn one_by_one() -> ImageData {
    rgba(1, 1, 4)
}

fn placed(id: u64, image: ImageData) -> CanvasImage {
    CanvasImage::new(id, CellRect::new(0, 0, 1, 1), image)
}

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

// ---- symbols: control characters ----

#[test]
fn check_rejects_every_kind_of_control_character_in_a_symbol() {
    for bad in [
        "\u{1b}",
        "\u{1b}[31m",
        "\u{7}",
        "\u{0}",
        "\n",
        "\r",
        "\t",
        "\u{7f}",
        "\u{80}",
        "\u{9b}",
        "\u{9f}",
        "a\u{1b}",
        "\u{9b}a",
        "e\u{301}\u{7}",
    ] {
        assert_eq!(
            with_symbol(3, 1, bad).check(),
            Err(CanvasIssue::CellControl { x: 1, y: 0 }),
            "{bad:?}"
        );
    }
}

#[test]
fn check_rejects_the_kitty_placeholder_alone_or_with_marks() {
    for bad in [
        "\u{10EEEE}",
        "\u{10EEEE}\u{0305}",
        "\u{10EEEE}\u{0305}\u{0305}",
        "a\u{10EEEE}",
    ] {
        let r = with_symbol(3, 1, bad).check();
        assert!(
            matches!(
                r,
                Err(CanvasIssue::CellReserved { x: 1, y: 0 })
                    | Err(CanvasIssue::CellNotOneGrapheme { x: 1, y: 0 })
            ),
            "{bad:?}: {r:?}"
        );
    }
    assert_eq!(
        with_symbol(3, 1, "\u{10EEEE}").check(),
        Err(CanvasIssue::CellReserved { x: 1, y: 0 })
    );
    assert_eq!(
        with_symbol(3, 1, "\u{10EEEE}\u{0305}\u{0305}").check(),
        Err(CanvasIssue::CellReserved { x: 1, y: 0 })
    );
    assert!(!konoma_plugin::view::CanvasCell::is_plain_symbol(
        "\u{10EEEE}"
    ));
}

#[test]
fn set_string_skips_the_kitty_placeholder_and_never_joins_it() {
    let mut c = konoma_plugin::view::CanvasView::new(6, 1);
    let took = c.set_string(
        0,
        0,
        "a\u{10EEEE}\u{0305}b\u{200D}\u{10EEEE}c",
        konoma_plugin::Style::new(),
    );
    assert!(c.check().is_ok());
    assert!(c.cells.iter().all(|x| !x.symbol.contains('\u{10EEEE}')));
    assert!(took <= 4, "{took}");
}

#[test]
fn check_accepts_ordinary_symbols() {
    for good in [
        " ",
        "a",
        "~",
        "é",
        "e\u{301}",
        "日",
        "🎉",
        "👨\u{200D}👩\u{200D}👧",
        "한",
        "❤\u{FE0F}",
    ] {
        let mut c = CanvasView::new(4, 1);
        c.set_string(0, 0, good, Style::default());
        assert_eq!(c.check(), Ok(()), "{good:?} written by set_string");
    }
}

// ---- symbols: length, one grapheme, width ----

fn marks(n_three: usize, n_two: usize) -> String {
    let mut s = String::from("e");
    s.extend(std::iter::repeat_n('\u{20D0}', n_three));
    s.extend(std::iter::repeat_n('\u{0301}', n_two));
    s
}

#[test]
fn check_limits_a_symbol_to_32_bytes() {
    assert_eq!(CANVAS_MAX_SYMBOL_BYTES, 32);
    let at = marks(9, 2);
    assert_eq!(at.len(), 32);
    assert_eq!(
        with_symbol(2, 0, &at).check(),
        Ok(()),
        "32 bytes is the limit"
    );
    let over = marks(10, 1);
    assert_eq!(over.len(), 33);
    assert_eq!(
        with_symbol(2, 0, &over).check(),
        Err(CanvasIssue::CellSymbolTooLong { x: 0, y: 0 })
    );
    // A huge symbol is refused by its length alone.
    let huge = "e".to_string() + &"\u{0301}".repeat(1 << 18);
    assert_eq!(
        with_symbol(2, 1, &huge).check(),
        Err(CanvasIssue::CellSymbolTooLong { x: 1, y: 0 })
    );
}

#[test]
fn check_requires_a_single_grapheme() {
    for bad in ["ab", "a ", "日本", "e\u{301}x", "a\u{200B}", "🎉🎉"] {
        assert_eq!(
            with_symbol(5, 2, bad).check(),
            Err(CanvasIssue::CellNotOneGrapheme { x: 2, y: 0 }),
            "{bad:?}"
        );
    }
}

#[test]
fn check_requires_a_width_of_one_or_two() {
    // A combining mark alone has no width.
    assert_eq!(
        with_symbol(3, 0, "\u{301}").check(),
        Err(CanvasIssue::CellBadWidth { x: 0, y: 0 })
    );
    // One grapheme wider than two columns: a prepended mark (U+0600, one column) before a wide
    // character is a single grapheme of three columns.
    use unicode_segmentation::UnicodeSegmentation;
    use unicode_width::UnicodeWidthStr;
    let three = "\u{0600}日";
    assert_eq!(three.graphemes(true).count(), 1);
    assert_eq!(three.width(), 3);
    assert_eq!(
        with_symbol(4, 0, three).check(),
        Err(CanvasIssue::CellBadWidth { x: 0, y: 0 })
    );
}

// ---- wide characters and continuations ----

#[test]
fn check_wants_the_continuation_after_a_wide_character() {
    // Correct pair.
    let mut ok = CanvasView::new(3, 1);
    ok.cell_mut(0, 0).unwrap().symbol = "日".into();
    ok.cell_mut(1, 0).unwrap().symbol = String::new();
    assert_eq!(ok.check(), Ok(()));
    // Wide, then an ordinary cell.
    assert_eq!(
        with_symbol(3, 0, "日").check(),
        Err(CanvasIssue::WideWithoutContinuation { x: 0, y: 0 })
    );
    // Wide in the last column: nowhere for the right half.
    assert_eq!(
        with_symbol(3, 2, "日").check(),
        Err(CanvasIssue::WideWithoutContinuation { x: 2, y: 0 })
    );
}

#[test]
fn check_refuses_a_continuation_that_follows_nothing_wide() {
    // At the start of a row.
    assert_eq!(
        with_symbol(3, 0, "").check(),
        Err(CanvasIssue::StrayContinuation { x: 0, y: 0 })
    );
    // After a narrow character.
    assert_eq!(
        with_symbol(3, 1, "").check(),
        Err(CanvasIssue::StrayContinuation { x: 1, y: 0 })
    );
    // Two continuations after one wide character.
    let mut two = CanvasView::new(4, 1);
    two.cell_mut(0, 0).unwrap().symbol = "日".into();
    two.cell_mut(1, 0).unwrap().symbol = String::new();
    two.cell_mut(2, 0).unwrap().symbol = String::new();
    assert_eq!(
        two.check(),
        Err(CanvasIssue::StrayContinuation { x: 2, y: 0 })
    );
}

#[test]
fn check_does_not_pair_a_wide_character_across_rows() {
    // A wide character in the last column of row 0 and a continuation at the start of row 1.
    let mut c = CanvasView::new(2, 2);
    c.cell_mut(1, 0).unwrap().symbol = "日".into();
    c.cell_mut(0, 1).unwrap().symbol = String::new();
    assert_eq!(
        c.check(),
        Err(CanvasIssue::WideWithoutContinuation { x: 1, y: 0 })
    );
}

#[test]
fn issues_report_the_row() {
    let mut c = CanvasView::new(3, 3);
    c.cell_mut(2, 2).unwrap().symbol = "\u{1b}".into();
    assert_eq!(c.check(), Err(CanvasIssue::CellControl { x: 2, y: 2 }));
}

// ---- set_string only produces cells that pass ----

#[test]
fn set_string_keeps_a_cell_within_32_bytes() {
    let mut c = CanvasView::new(6, 1);
    let long = "e".to_string() + &"\u{0301}".repeat(100);
    let n = c.set_string(0, 0, &long, Style::default());
    assert_eq!(n, 1);
    let s = &c.cell(0, 0).unwrap().symbol;
    assert!(s.len() <= CANVAS_MAX_SYMBOL_BYTES, "{} bytes", s.len());
    assert!(s.starts_with('e'));
    assert_eq!(c.check(), Ok(()));
}

#[test]
fn set_string_drops_a_zero_width_character_that_would_split_the_grapheme() {
    let mut c = CanvasView::new(6, 1);
    // U+200B (zero width space) is its own grapheme: it may not ride in the cell of `a`.
    c.set_string(0, 0, "a\u{200B}b", Style::default());
    assert_eq!(c.cell(0, 0).unwrap().symbol, "a");
    assert_eq!(c.cell(1, 0).unwrap().symbol, "b");
    assert_eq!(c.check(), Ok(()));
}

#[test]
fn set_string_never_changes_the_columns_a_cell_takes() {
    // A variation selector that would widen a narrow symbol is dropped, so the cell count the
    // caller is told matches the cells written.
    let mut c = CanvasView::new(6, 1);
    let n = c.set_string(0, 0, "\u{2764}\u{FE0F}x", Style::default());
    assert_eq!(c.check(), Ok(()));
    assert_eq!(n, 2);
    assert_eq!(c.cell(0, 0).unwrap().symbol, "\u{2764}");
    assert_eq!(c.cell(1, 0).unwrap().symbol, "x");
}

#[test]
fn set_string_output_always_passes_check() {
    let pieces = [
        "a", "日", "🎉", "\u{301}", "\u{200D}", "\u{200B}", "\u{FE0F}", "\u{1b}", "\n", "\u{9b}",
        "👨", "👩", "e", "한", "\u{FDFD}", "\u{20D0}", "\u{2764}", " ",
    ];
    // Every pair and triple of pieces, written at every start column of a small canvas.
    for a in pieces {
        for b in pieces {
            for c3 in pieces {
                let s = format!("{a}{b}{c3}");
                for x in 0..4u16 {
                    let mut c = CanvasView::new(5, 1);
                    c.set_string(0, 0, "日日x", Style::default());
                    c.set_string(x, 0, &s, Style::default());
                    assert_eq!(c.check(), Ok(()), "{s:?} at {x}");
                }
            }
        }
    }
}

// ---- area ----

#[test]
fn check_bounds_the_canvas_area() {
    assert_eq!(CANVAS_MAX_SIDE, 4096);
    assert_eq!(CANVAS_MAX_CELLS, 1 << 22);
    assert_eq!(CanvasView::new(4096, 1).check(), Ok(()));
    assert_eq!(CanvasView::new(1, 4096).check(), Ok(()));
    // The area is refused before the cells are looked at: no cells are needed to say so.
    let mut big = CanvasView::new(0, 0);
    for (cols, rows) in [
        (4097, 1),
        (1, 4097),
        (u16::MAX, u16::MAX),
        (4096, 1025),
        (2049, 2049),
    ] {
        big.cols = cols;
        big.rows = rows;
        assert_eq!(
            big.check(),
            Err(CanvasIssue::AreaTooLarge { cols, rows }),
            "{cols}x{rows}"
        );
    }
    // Exactly the cell limit is within the area rule (the cells are then counted).
    big.cols = 4096;
    big.rows = 1024;
    assert!(matches!(
        big.check(),
        Err(CanvasIssue::CellCount { expected, actual: 0 }) if expected == CANVAS_MAX_CELLS
    ));
}

// ---- placements and picture ids ----

#[test]
fn check_bounds_placements_and_distinct_ids() {
    assert_eq!(CANVAS_MAX_PLACEMENTS, 4096);
    assert_eq!(CANVAS_MAX_PICTURES, 256);
    // 4096 placements of 256 ids: the limits exactly.
    let mut c = CanvasView::new(1, 1);
    for n in 0..CANVAS_MAX_PLACEMENTS {
        c.images.push(CanvasImage::unchanged(
            (n % 256) as u64,
            CellRect::new(0, 0, 1, 1),
        ));
    }
    assert_eq!(c.check(), Ok(()));
    // One placement more.
    c.images
        .push(CanvasImage::unchanged(0, CellRect::new(0, 0, 1, 1)));
    assert_eq!(
        c.check(),
        Err(CanvasIssue::TooManyPlacements {
            count: CANVAS_MAX_PLACEMENTS + 1
        })
    );
    // 257 distinct ids.
    let mut d = CanvasView::new(1, 1);
    for id in 0..=CANVAS_MAX_PICTURES as u64 {
        d.images
            .push(CanvasImage::unchanged(id, CellRect::new(0, 0, 1, 1)));
    }
    assert_eq!(
        d.check(),
        Err(CanvasIssue::TooManyPictures {
            count: CANVAS_MAX_PICTURES + 1
        })
    );
    // 256 distinct ids are fine.
    d.images.pop();
    assert_eq!(d.check(), Ok(()));
}

// ---- Rgba ----

#[test]
fn check_wants_rgba_to_be_exactly_width_height_four() {
    let ok = CanvasView::new(1, 1).image(placed(1, rgba(2, 3, 24)));
    assert_eq!(ok.check(), Ok(()));
    for (w, h, bytes) in [(2, 3, 23), (2, 3, 25), (2, 3, 0), (1, 1, 3), (1, 1, 5)] {
        assert_eq!(
            CanvasView::new(1, 1)
                .image(placed(1, rgba(w, h, bytes)))
                .check(),
            Err(CanvasIssue::ImageRgbaSize { index: 0 }),
            "{w}x{h} with {bytes} bytes"
        );
    }
    // A zero side is never a picture (even with the "right" zero bytes).
    for (w, h) in [(0, 0), (0, 5), (5, 0)] {
        assert_eq!(
            CanvasView::new(1, 1)
                .image(placed(1, rgba(w, h, 0)))
                .check(),
            Err(CanvasIssue::ImageRgbaSize { index: 0 }),
            "{w}x{h}"
        );
    }
}

#[test]
fn check_limits_the_side_of_rgba() {
    assert_eq!(CANVAS_MAX_RGBA_SIDE, 4096);
    let wide = rgba(CANVAS_MAX_RGBA_SIDE, 1, CANVAS_MAX_RGBA_SIDE as usize * 4);
    assert_eq!(CanvasView::new(1, 1).image(placed(1, wide)).check(), Ok(()));
    let tall = rgba(1, CANVAS_MAX_RGBA_SIDE, CANVAS_MAX_RGBA_SIDE as usize * 4);
    assert_eq!(CanvasView::new(1, 1).image(placed(1, tall)).check(), Ok(()));
    // Over the side: refused by the side, whatever the byte count (no 4 GiB buffer is needed).
    for (w, h) in [
        (CANVAS_MAX_RGBA_SIDE + 1, 1),
        (1, CANVAS_MAX_RGBA_SIDE + 1),
        (u32::MAX, u32::MAX),
    ] {
        assert_eq!(
            CanvasView::new(1, 1)
                .image(placed(1, rgba(w, h, 4)))
                .check(),
            Err(CanvasIssue::ImageTooLarge { index: 0 }),
            "{w}x{h}"
        );
    }
}

#[test]
fn check_applies_the_rgba_rules_inside_frames_and_names_the_image() {
    let frames = ImageData::Frames(vec![(one_by_one(), ms(40)), (rgba(1, 1, 7), ms(40))]);
    let c = CanvasView::new(2, 1)
        .image(placed(1, one_by_one()))
        .image(CanvasImage::new(2, CellRect::new(1, 0, 1, 1), frames));
    assert_eq!(c.check(), Err(CanvasIssue::ImageRgbaSize { index: 1 }));
    let big = ImageData::Frames(vec![(rgba(CANVAS_MAX_RGBA_SIDE + 1, 1, 4), ms(40))]);
    assert_eq!(
        CanvasView::new(1, 1).image(placed(1, big)).check(),
        Err(CanvasIssue::ImageTooLarge { index: 0 })
    );
}

// ---- Frames ----

#[test]
fn check_refuses_nested_frames() {
    let inner = ImageData::Frames(vec![(one_by_one(), ms(40))]);
    let outer = ImageData::Frames(vec![(inner, ms(40))]);
    assert_eq!(
        CanvasView::new(1, 1).image(placed(1, outer)).check(),
        Err(CanvasIssue::ImageNestedFrames { index: 0 })
    );
}

#[test]
fn check_limits_the_frame_count() {
    assert_eq!(CANVAS_MAX_FRAMES, 5000);
    let frame = one_by_one();
    let at = ImageData::Frames(vec![(frame.clone(), ms(40)); CANVAS_MAX_FRAMES]);
    assert_eq!(CanvasView::new(1, 1).image(placed(1, at)).check(), Ok(()));
    let over = ImageData::Frames(vec![(frame, ms(40)); CANVAS_MAX_FRAMES + 1]);
    assert_eq!(
        CanvasView::new(1, 1).image(placed(1, over)).check(),
        Err(CanvasIssue::ImageTooManyFrames { index: 0 })
    );
}

#[test]
fn check_does_not_enforce_frame_delays() {
    // The host raises short delays itself.
    let frames = ImageData::Frames(vec![(one_by_one(), Duration::ZERO), (one_by_one(), ms(1))]);
    assert_eq!(
        CanvasView::new(1, 1).image(placed(1, frames)).check(),
        Ok(())
    );
}

// ---- an unchanged picture ----

#[test]
fn an_unchanged_picture_is_plain_data_that_passes_check() {
    let c = CanvasView::new(4, 2).image(CanvasImage::unchanged(9, CellRect::new(1, 0, 2, 2)));
    assert_eq!(c.check(), Ok(()));
    let im = &c.images[0];
    assert!(im.image.is_none());
    assert_eq!(im.rect(), CellRect::new(1, 0, 2, 2));
    // An unchanged picture is still placed inside the canvas.
    let out = CanvasView::new(4, 2).image(CanvasImage::unchanged(9, CellRect::new(3, 0, 2, 2)));
    assert_eq!(out.check(), Err(CanvasIssue::ImageOutOfBounds { index: 0 }));
}

// ---- constructors and builders (the types are non_exhaustive) ----

#[test]
fn canvas_image_new_and_rect() {
    let im = CanvasImage::new(7, CellRect::new(1, 2, 3, 4), one_by_one());
    assert_eq!((im.id, im.x, im.y, im.cols, im.rows), (7, 1, 2, 3, 4));
    assert!(im.image.is_some());
    assert_eq!(im.rect(), CellRect::new(1, 2, 3, 4));
}

#[test]
fn term_caps_builders() {
    let d = TermCaps::new();
    assert_eq!(d, TermCaps::default());
    assert!(!d.pixels);
    assert_eq!(d.colors, ColorDepth::Ansi16);
    let t = TermCaps::new().pixels(true).colors(ColorDepth::TrueColor);
    assert!(t.pixels);
    assert_eq!(t.colors, ColorDepth::TrueColor);
    assert!(!TermCaps::new().colors(ColorDepth::Ansi256).pixels);
}

#[test]
fn view_req_builders() {
    let r = ViewReq::new(10, 5);
    assert_eq!(
        (r.cols, r.rows, r.repr.clone(), r.page, r.cell_px),
        (10, 5, None, None, None)
    );
    let r = ViewReq::new(10, 5).repr("text").page(3).cell_px(8, 16);
    assert_eq!(r.repr.as_deref(), Some("text"));
    assert_eq!(r.page, Some(3));
    assert_eq!(r.cell_px, Some((8, 16)));
}

#[test]
fn non_exhaustive_enums_are_matched_with_a_wildcard() {
    // Compiles only with `_` arms from outside the crate; this pins that the arms exist and the
    // known values still map.
    let name = |k: &KeyCode| match k {
        KeyCode::Char(_) => "char",
        KeyCode::Enter => "enter",
        _ => "other",
    };
    assert_eq!(name(&Key::char('a').code), "char");
    assert_eq!(name(&Key::new(KeyCode::Esc).code), "other");
    let depth = |d: ColorDepth| match d {
        ColorDepth::TrueColor => "true",
        _ => "less",
    };
    assert_eq!(depth(ColorDepth::Ansi256), "less");
    let kind = |k: ViewKind| match k {
        ViewKind::Canvas => "canvas",
        _ => "other",
    };
    assert_eq!(kind(ViewKind::Text), "other");
    let issue = |i: &CanvasIssue| match i {
        CanvasIssue::CellControl { .. } => "control",
        _ => "other",
    };
    assert_eq!(
        issue(&CanvasIssue::StrayContinuation { x: 0, y: 0 }),
        "other"
    );
    let v = View::Canvas(CanvasView::new(1, 1));
    assert!(matches!(&v, View::Canvas(_)));
}

#[test]
fn the_input_key_code_is_exported_plainly() {
    // At the crate root and in the prelude, by its own name.
    let a: konoma_plugin::KeyCode = KeyCode::Enter;
    let b: konoma_plugin::key::KeyCode = a.clone();
    assert_eq!(a, b);
}

#[test]
fn every_issue_has_a_message() {
    let issues = [
        CanvasIssue::AreaTooLarge { cols: 1, rows: 2 },
        CanvasIssue::CellControl { x: 3, y: 4 },
        CanvasIssue::CellSymbolTooLong { x: 3, y: 4 },
        CanvasIssue::CellNotOneGrapheme { x: 3, y: 4 },
        CanvasIssue::CellBadWidth { x: 3, y: 4 },
        CanvasIssue::WideWithoutContinuation { x: 3, y: 4 },
        CanvasIssue::StrayContinuation { x: 3, y: 4 },
        CanvasIssue::TooManyPlacements { count: 5 },
        CanvasIssue::TooManyPictures { count: 5 },
        CanvasIssue::ImageRgbaSize { index: 6 },
        CanvasIssue::ImageTooLarge { index: 6 },
        CanvasIssue::ImageNestedFrames { index: 6 },
        CanvasIssue::ImageTooManyFrames { index: 6 },
    ];
    let mut seen = std::collections::HashSet::new();
    for i in issues {
        let m = i.to_string();
        assert!(!m.is_empty());
        assert!(seen.insert(m), "{i:?} shares a message");
    }
}

#[test]
fn a_default_cell_is_a_valid_symbol() {
    let c = CanvasCell::default();
    assert_eq!(c.symbol, " ");
    assert_eq!(CanvasView::new(3, 3).check(), Ok(()));
}

#[test]
fn every_ascii_control_character_is_refused_and_every_printable_one_accepted() {
    // The single-byte fast path must agree with the general path at both of its edges (U+001F is
    // the last C0 control, U+007F the DEL), not only for the few controls tried elsewhere.
    for b in 0u8..=0x7f {
        let s = char::from(b).to_string();
        let printable = (0x20..0x7f).contains(&b);
        assert_eq!(CanvasCell::is_plain_symbol(&s), printable, "{b:#04x}");
        let got = with_symbol(3, 1, &s).check();
        if printable {
            assert_eq!(got, Ok(()), "{b:#04x}");
        } else {
            assert_eq!(
                got,
                Err(CanvasIssue::CellControl { x: 1, y: 0 }),
                "{b:#04x}"
            );
        }
    }
}

// ---- clipped: what the host keeps of a canvas is bounded by the size it asked for ----

fn text_at(c: &CanvasView, x: u16, y: u16) -> String {
    c.cells[y as usize * c.cols as usize + x as usize]
        .symbol
        .clone()
}

#[test]
fn clipped_keeps_the_top_left_part_and_the_cells_in_it() {
    let mut c = CanvasView::new(10, 6);
    c.set_string(0, 0, "ab", Style::default());
    c.set_string(3, 2, "x", Style::default());
    c.set_string(9, 5, "z", Style::default());
    let cut = c.clipped(4, 3).unwrap();
    assert_eq!((cut.cols, cut.rows), (4, 3));
    assert_eq!(
        cut.cells.len(),
        12,
        "the cell vector is cut, not only the size"
    );
    assert_eq!(text_at(&cut, 0, 0), "a");
    assert_eq!(text_at(&cut, 1, 0), "b");
    assert_eq!(text_at(&cut, 3, 2), "x");
    assert_eq!(cut.check(), Ok(()));
}

#[test]
fn clipped_returns_a_canvas_that_fits_as_it_is() {
    let mut c = CanvasView::new(5, 2);
    c.set_string(0, 0, "hi", Style::default());
    c.redraw_after = Some(ms(100));
    c.text_input = true;
    let same = c.clone().clipped(5, 2).unwrap();
    assert_eq!(same.cells, c.cells);
    assert_eq!((same.cols, same.rows), (5, 2));
    let wider = c.clone().clipped(500, 500).unwrap();
    assert_eq!(wider.cells, c.cells);
    // The other fields of a cut canvas are kept too.
    let cut = c.clipped(3, 1).unwrap();
    assert_eq!(cut.redraw_after, Some(ms(100)));
    assert!(cut.text_input);
}

#[test]
fn clipped_to_nothing_is_an_empty_canvas() {
    let c = CanvasView::new(8, 8);
    for (mc, mr) in [(0, 5), (5, 0), (0, 0)] {
        let cut = c.clone().clipped(mc, mr).unwrap();
        assert_eq!(cut.cells.len(), mc as usize * mr as usize);
        assert_eq!(cut.check(), Ok(()), "{mc}x{mr}");
    }
}

#[test]
fn clipped_blanks_a_wide_character_cut_by_the_right_edge() {
    let mut c = CanvasView::new(6, 1);
    // Wide characters at columns 0-1, 2-3 and 4-5.
    c.set_string(0, 0, "世界語", Style::default());
    assert_eq!(c.check(), Ok(()));
    // Keeping 3 columns cuts "界" (2-3) in half: its left half cannot stay alone.
    let cut = c.clone().clipped(3, 1).unwrap();
    assert_eq!(text_at(&cut, 0, 0), "世");
    assert_eq!(text_at(&cut, 1, 0), "");
    assert_eq!(
        text_at(&cut, 2, 0),
        " ",
        "the half-cut wide character is blank"
    );
    assert_eq!(
        cut.check(),
        Ok(()),
        "never a wide character without its continuation"
    );
    // Keeping 4 columns cuts between characters: nothing is blanked.
    let even = c.clipped(4, 1).unwrap();
    assert_eq!(text_at(&even, 2, 0), "界");
    assert_eq!(even.check(), Ok(()));
}

#[test]
fn clipped_keeps_the_placements_that_start_inside_whole_and_drops_those_wholly_outside() {
    let mut c = CanvasView::new(20, 10);
    c.images = vec![
        CanvasImage::new(1, CellRect::new(0, 0, 4, 2), one_by_one()),
        CanvasImage::new(2, CellRect::new(6, 0, 4, 2), one_by_one()), // runs past the right edge
        CanvasImage::new(3, CellRect::new(0, 4, 4, 4), one_by_one()), // runs past the bottom
        CanvasImage::new(4, CellRect::new(15, 8, 2, 2), one_by_one()), // wholly outside
        CanvasImage::unchanged(5, CellRect::new(6, 3, 2, 2)),         // exactly at the corner
        CanvasImage::new(6, CellRect::new(8, 5, 2, 2), one_by_one()), // starts on the edge: outside
    ];
    let cut = c.clipped(8, 5).unwrap();
    let ids: Vec<u64> = cut.images.iter().map(|i| i.id).collect();
    assert_eq!(ids, vec![1, 2, 3, 5]);
    // Kept whole, rectangles untouched.
    assert_eq!(cut.images[1].rows, 2);
    assert_eq!(cut.images[2].rows, 4);
    // `clipped` validated it already; plain `check` still wants every placement inside.
    assert!(matches!(
        cut.check(),
        Err(CanvasIssue::ImageOutOfBounds { .. })
    ));
}

#[test]
fn clipped_judges_the_placement_bounds_on_the_canvas_as_sent() {
    let mut c = CanvasView::new(20, 10);
    c.images = vec![CanvasImage::new(
        1,
        CellRect::new(18, 0, 4, 2),
        one_by_one(),
    )];
    assert!(matches!(
        c.clone().clipped(8, 5),
        Err(CanvasIssue::ImageOutOfBounds { index: 0 })
    ));
    // Also when the canvas already fits the cut.
    assert!(matches!(
        c.clipped(50, 50),
        Err(CanvasIssue::ImageOutOfBounds { index: 0 })
    ));
}

#[test]
fn clipped_still_refuses_what_is_wrong_with_the_size_of_what_was_sent() {
    // The size-only rules are judged on the canvas as it was sent, before anything is cut.
    let mut c = CanvasView::new(4, 4);
    c.cells.pop();
    assert!(matches!(
        c.clipped(2, 2),
        Err(CanvasIssue::CellCount {
            expected: 16,
            actual: 15
        })
    ));
    let mut big = CanvasView::new(0, 0);
    big.cols = 4097;
    big.rows = 1;
    assert_eq!(
        big.clipped(10, 10).unwrap_err(),
        CanvasIssue::AreaTooLarge {
            cols: 4097,
            rows: 1
        }
    );
    let mut many = CanvasView::new(4, 4);
    many.images = (0..=CANVAS_MAX_PLACEMENTS as u64)
        .map(|i| CanvasImage::unchanged(i % 8, CellRect::new(0, 0, 1, 1)))
        .collect();
    assert!(matches!(
        many.clipped(2, 2),
        Err(CanvasIssue::TooManyPlacements { .. })
    ));
}

#[test]
fn clipped_leaves_bad_cells_outside_the_cut_unjudged_and_refuses_those_inside() {
    let mut c = CanvasView::new(100, 100);
    c.cell_mut(90, 90).unwrap().symbol = "\u{1b}".to_string();
    let cut = c.clone().clipped(10, 10).unwrap();
    assert_eq!(cut.check(), Ok(()), "a bad cell that was cut away is gone");
    c.cell_mut(5, 5).unwrap().symbol = "\u{1b}".to_string();
    assert_eq!(
        c.clone().clipped(10, 10).unwrap_err(),
        CanvasIssue::CellControl { x: 5, y: 5 },
        "clipped validates the kept cells itself"
    );
    // Also when nothing is cut.
    assert_eq!(
        c.clipped(500, 500).unwrap_err(),
        CanvasIssue::CellControl { x: 5, y: 5 }
    );
}

/// A canvas 20x10 cut to 8x5, with `bad` placed wholly outside the kept part (at x = 12).
fn cut_with_outside(bad: Vec<CanvasImage>) -> Result<CanvasView, CanvasIssue> {
    let mut c = CanvasView::new(20, 10);
    c.images = bad;
    c.clipped(8, 5)
}

#[test]
fn a_violation_only_in_a_placement_starting_outside_the_kept_part_is_refused() {
    let zero = cut_with_outside(vec![CanvasImage::unchanged(1, CellRect::new(12, 0, 0, 2))]);
    assert!(matches!(zero, Err(CanvasIssue::ImageZeroSize { index: 0 })));
    let mismatch = cut_with_outside(vec![
        CanvasImage::unchanged(1, CellRect::new(0, 0, 2, 2)),
        CanvasImage::unchanged(1, CellRect::new(12, 0, 3, 2)),
    ]);
    assert!(matches!(
        mismatch,
        Err(CanvasIssue::ImageSizeMismatch { index: 1 })
    ));
    let oob = cut_with_outside(vec![CanvasImage::unchanged(1, CellRect::new(18, 0, 4, 2))]);
    assert!(matches!(
        oob,
        Err(CanvasIssue::ImageOutOfBounds { index: 0 })
    ));
    let bad_data = cut_with_outside(vec![CanvasImage::new(
        1,
        CellRect::new(12, 0, 2, 2),
        ImageData::Rgba {
            width: 4,
            height: 4,
            pixels: std::sync::Arc::new(vec![0; 3]),
        },
    )]);
    assert!(bad_data.is_err(), "bad picture data outside is refused");
}

#[test]
fn too_many_pictures_counts_the_placements_as_sent() {
    let imgs = (0..=CANVAS_MAX_PICTURES as u64)
        .map(|i| CanvasImage::unchanged(i, CellRect::new(12, 0, 1, 1)))
        .collect();
    assert!(matches!(
        cut_with_outside(imgs),
        Err(CanvasIssue::TooManyPictures { .. })
    ));
}
