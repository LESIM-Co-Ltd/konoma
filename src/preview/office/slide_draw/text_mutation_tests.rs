//! Tests from mutation testing of the text layout (`text.rs`): each pins a behaviour a mutant
//! (flipped comparison, swapped operand, removed branch) used to change without any test noticing.

use super::*;

fn size_px_of(run_size_pt: f64) -> f64 {
    let l = lay(vec![p("x", run_size_pt)], 500.0, 300.0);
    l.lines[0].frags[0].style.size_px
}

#[test]
fn a_run_size_that_is_not_a_positive_number_is_the_default_size() {
    // the default is 18 pt = 24 px
    for pt in [0.0, -5.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert!(
            (size_px_of(pt) - 24.0).abs() < 1e-9,
            "{pt}: {}",
            size_px_of(pt)
        );
    }
    assert!((size_px_of(12.0) - 16.0).abs() < 1e-9);
    // clamped to 0.5 pt .. 3000 pt
    assert!((size_px_of(0.1) - 0.5 * PX_PT).abs() < 1e-9);
    assert!((size_px_of(1e9) - 3000.0 * PX_PT).abs() < 1e-9);
}

#[test]
fn a_pattern_fill_draws_its_foreground_colour() {
    let mut r = Run::text("x", 20.0);
    r.fill = Fill::Pattern {
        preset: "pct50".into(),
        fg: Rgba::rgb(1, 2, 3),
        bg: Rgba::rgb(200, 200, 200),
    };
    let l = lay(
        vec![Paragraph {
            runs: vec![r],
            ..Default::default()
        }],
        300.0,
        100.0,
    );
    assert_eq!(l.lines[0].frags[0].style.color, Rgba::rgb(1, 2, 3));
}

#[test]
fn roman_numerals_stop_at_3999() {
    assert_eq!(autonum_text("romanLcPeriod", 3999), "mmmcmxcix.");
    assert_eq!(autonum_text("romanUcPeriod", 3999), "MMMCMXCIX.");
    assert_eq!(autonum_text("romanLcPeriod", 4000), "4000.");
    assert_eq!(autonum_text("romanUcParenR", 4000), "4000)");
    assert_eq!(autonum_text("romanLcPeriod", 3888), "mmmdccclxxxviii.");
}

#[test]
fn the_runs_of_every_paragraph_keep_their_own_style() {
    let l = lay(
        vec![p("one", 10.0), p("two", 30.0), p("three", 20.0)],
        600.0,
        400.0,
    );
    let sizes: Vec<f64> = l.lines.iter().map(|ln| ln.frags[0].style.size_px).collect();
    assert_eq!(sizes, vec![10.0 * PX_PT, 30.0 * PX_PT, 20.0 * PX_PT]);
}

fn bullet_char() -> Bullet {
    Bullet {
        kind: BulletKind::Char("•".into()),
        font: None,
        color: None,
        size: BulletSize::FollowText,
    }
}

#[test]
fn a_bullet_takes_its_size_from_the_first_run_that_is_not_a_line_break() {
    let br = Run {
        kind: RunKind::LineBreak,
        size_pt: 10.0,
        ..Run::text("", 10.0)
    };
    let pa = Paragraph {
        runs: vec![br, Run::text("big", 40.0)],
        bullet: Some(bullet_char()),
        mar_l: 342_900.0,
        indent: -342_900.0,
        ..Default::default()
    };
    let l = lay(vec![pa], 600.0, 400.0);
    match &l.bullets[0] {
        BulletDraw::Text(f) => assert!(
            (f.style.size_px - 40.0 * PX_PT).abs() < 0.01,
            "{}",
            f.style.size_px
        ),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_bullet_paragraph_with_only_a_line_break_still_has_its_bullet_and_an_empty_one_has_none() {
    let br = Run {
        kind: RunKind::LineBreak,
        ..Run::text("", 20.0)
    };
    let only_break = Paragraph {
        runs: vec![br],
        bullet: Some(bullet_char()),
        ..Default::default()
    };
    let l = lay(vec![only_break], 600.0, 400.0);
    assert_eq!(l.bullets.len(), 1);
    let empty = Paragraph {
        bullet: Some(bullet_char()),
        ..Default::default()
    };
    let l = lay(vec![empty], 600.0, 400.0);
    assert_eq!(l.bullets.len(), 0);
    // with text, of course
    let with_text = Paragraph {
        bullet: Some(bullet_char()),
        ..p("x", 20.0)
    };
    assert_eq!(lay(vec![with_text], 600.0, 400.0).bullets.len(), 1);
}

#[test]
fn numbering_continues_over_an_empty_paragraph_and_restarts_after_text() {
    let l = lay(
        vec![
            numbered(0, "a", "arabicPeriod", 1),
            Paragraph::default(),
            numbered(0, "b", "arabicPeriod", 1),
            p("plain text", 18.0),
            numbered(0, "c", "arabicPeriod", 1),
        ],
        500.0,
        900.0,
    );
    assert_eq!(bullet_texts(&l), vec!["1.", "2.", "1."]);
}

#[test]
fn a_picture_bullet_is_the_size_of_the_font() {
    let pic = |font_scale: f64| {
        let pa = Paragraph {
            bullet: Some(Bullet {
                kind: BulletKind::Picture(ImageFill::stretch("b")),
                font: None,
                color: None,
                size: BulletSize::FollowText,
            }),
            ..p("x", 20.0)
        };
        let mut b = body(vec![pa]);
        b.autofit = AutoFit::Normal {
            font_scale,
            ln_spc_reduction: 0.0,
        };
        layout(&b, 600.0, 300.0)
    };
    for (scale, want) in [(1.0, 20.0 * PX_PT), (0.5, 10.0 * PX_PT)] {
        match &pic(scale).bullets[0] {
            BulletDraw::Picture { size, .. } => {
                assert!((size - want).abs() < 0.01, "{scale}: {size}")
            }
            other => panic!("{other:?}"),
        }
    }
}

#[test]
fn the_text_of_a_bulleted_paragraph_starts_after_the_bullet_and_its_gap() {
    // margin 0, no indent: the text starts where the bullet and 0.3 of its size end
    let pa = Paragraph {
        bullet: Some(bullet_char()),
        ..p("text", 20.0)
    };
    let l = lay(vec![pa], 600.0, 300.0);
    let b = match &l.bullets[0] {
        BulletDraw::Text(f) => f.clone(),
        other => panic!("{other:?}"),
    };
    assert!(b.x.abs() < 1e-9);
    let want = b.width + 0.3 * b.style.size_px;
    assert!(
        (l.lines[0].frags[0].x - want).abs() < 0.01,
        "{} vs {want}",
        l.lines[0].frags[0].x
    );
    // a hanging indent: the bullet at margin + indent, the text at the margin when the bullet fits
    let pa = Paragraph {
        bullet: Some(bullet_char()),
        mar_l: 952_500.0,   // 100 px
        indent: -285_750.0, // -30 px
        ..p("text", 20.0)
    };
    let l = lay(vec![pa], 600.0, 300.0);
    match &l.bullets[0] {
        BulletDraw::Text(f) => assert!((f.x - 70.0).abs() < 0.01, "{}", f.x),
        other => panic!("{other:?}"),
    }
    assert!((l.lines[0].frags[0].x - 100.0).abs() < 0.01);
    // an indent that would put the bullet left of the box is held at the box
    let pa = Paragraph {
        bullet: Some(bullet_char()),
        mar_l: 95_250.0,    // 10 px
        indent: -285_750.0, // -30 px
        ..p("text", 20.0)
    };
    let l = lay(vec![pa], 600.0, 300.0);
    match &l.bullets[0] {
        BulletDraw::Text(f) => assert!(f.x.abs() < 0.01, "{}", f.x),
        other => panic!("{other:?}"),
    }
}

#[test]
fn percentage_paragraph_spacing_loses_the_autofit_line_spacing_reduction() {
    let single = 1.2 * 20.0 * PX_PT;
    let pitch = |red: f64| {
        let mut a = p("a", 20.0);
        a.line_spacing = Spacing::Pct(1.0);
        a.spc_after = Spacing::Pct(0.5);
        let mut b = body(vec![a, p("b", 20.0)]);
        b.autofit = AutoFit::Normal {
            font_scale: 1.0,
            ln_spc_reduction: red,
        };
        let l = layout(&b, 600.0, 400.0);
        l.lines[1].baseline - l.lines[0].baseline
    };
    assert!((pitch(0.0) - (single + 0.5 * single)).abs() < 0.01);
    // with a reduction of 0.2 both the line (1.0 -> 0.8) and the space (0.5 -> 0.3) shrink; the
    // space is a share of the (reduced) line it follows
    let lh = 0.8 * single;
    assert!(
        (pitch(0.2) - (lh + 0.3 * lh)).abs() < 0.01,
        "{}",
        pitch(0.2)
    );
    // a reduction larger than the spacing leaves no negative space
    let mut a = p("a", 20.0);
    a.line_spacing = Spacing::Pct(1.0);
    a.spc_after = Spacing::Pct(0.1);
    let mut b = body(vec![a, p("b", 20.0)]);
    b.autofit = AutoFit::Normal {
        font_scale: 1.0,
        ln_spc_reduction: 0.4,
    };
    let l = layout(&b, 600.0, 400.0);
    let gap = l.lines[1].baseline - l.lines[0].baseline;
    assert!((gap - 0.6 * single).abs() < 0.01, "{gap}");
}

#[test]
fn a_break_after_an_opening_bracket_and_before_a_closing_one_is_still_a_break() {
    // a forced break ends the line even when the neighbouring character has a kinsoku rule
    let runs = |a: &str, b: &str| {
        let br = Run {
            kind: RunKind::LineBreak,
            ..Run::text("", 20.0)
        };
        vec![Run::text(a, 20.0), br, Run::text(b, 20.0)]
    };
    let l = lay(
        vec![Paragraph {
            runs: runs("あ（", "い"),
            ..Default::default()
        }],
        600.0,
        300.0,
    );
    assert_eq!(texts(&l), vec!["あ（", "い"]);
    let l = lay(
        vec![Paragraph {
            runs: runs("あ", "。い"),
            ..Default::default()
        }],
        600.0,
        300.0,
    );
    assert_eq!(texts(&l), vec!["あ", "。い"]);
}

#[test]
fn two_pieces_of_one_word_in_different_styles_are_not_split_by_wrapping() {
    // "ab Hel|lo": the word is "Hello" although two runs write it
    let bold = Run {
        bold: true,
        ..Run::text("Hel", 20.0)
    };
    let pa = Paragraph {
        runs: vec![Run::text("ab ", 20.0), bold, Run::text("lo", 20.0)],
        ..Default::default()
    };
    // a box that holds "ab Hel" but not "ab Hello"
    let one_line = lay(vec![pa.clone()], 600.0, 300.0);
    let full = line_w(&one_line.lines[0]);
    let hello_only = {
        let l = lay(vec![p("Hello", 20.0)], 600.0, 300.0);
        line_w(&l.lines[0])
    };
    let width = full - hello_only * 0.2;
    let l = lay(vec![pa], width, 300.0);
    assert_eq!(texts(&l), vec!["ab", "Hello"]);
}

#[test]
fn a_word_broken_between_characters_counts_the_letter_spacing_of_every_piece() {
    let mut r = Run::text("m".repeat(12), 20.0);
    r.spacing_pt = 10.0;
    let pa = Paragraph {
        runs: vec![r],
        ..Default::default()
    };
    let l = lay(vec![pa], 100.0, 600.0);
    // each piece is a letter and 13.3 px of spacing, 100 px hold three of them at most
    assert!(l.lines.len() >= 4, "{:?}", texts(&l));
    for line in &l.lines {
        assert!(line_text(line).chars().count() <= 3, "{:?}", texts(&l));
        assert!(line_w(line) <= 100.5, "{}", line_w(line));
    }
    assert_eq!(texts(&l).concat(), "m".repeat(12));
}

#[test]
fn a_closing_bracket_cut_off_a_long_word_does_not_start_a_line() {
    // the width of one "w" and of the bracket, measured
    let one = |s: &str| line_w(&lay(vec![p(s, 20.0)], 1000.0, 100.0).lines[0]);
    let w = one("w");
    // a box that holds exactly twelve "w" and not the bracket after them
    let width = 12.0 * w + 0.5 * one(")");
    let word = format!("{})", "w".repeat(12));
    let l = lay(vec![p(&word, 20.0)], width, 400.0);
    assert!(l.lines.len() >= 2, "{:?}", texts(&l));
    for line in &l.lines {
        assert!(!line_text(line).starts_with(')'), "{:?}", texts(&l));
    }
    assert_eq!(texts(&l).concat(), word);
    // the same with an opening bracket: it does not end a line
    let word = format!("{}(w", "w".repeat(11));
    let width = 12.0 * w - 0.2 * w;
    let l = lay(vec![p(&word, 20.0)], width, 400.0);
    for line in &l.lines {
        assert!(!line_text(line).ends_with('('), "{:?}", texts(&l));
    }
    assert_eq!(texts(&l).concat(), word);
}

#[test]
fn a_latin_word_and_a_cjk_character_may_be_split_where_the_line_ends() {
    // "ab word" fits but "ab wordあ" does not: the CJK character wraps alone
    let one = |s: &str| line_w(&lay(vec![p(s, 20.0)], 2000.0, 100.0).lines[0]);
    let width = one("ab word") + 0.5;
    let l = lay(vec![p("ab wordあいう", 20.0)], width, 400.0);
    assert_eq!(texts(&l)[0], "ab word", "{:?}", texts(&l));
    assert!(texts(&l)[1].starts_with('あ'), "{:?}", texts(&l));
}

#[test]
fn a_tab_after_text_goes_to_the_stop_after_the_text_not_the_one_after_the_line_start() {
    // 6 "m" are wider than one inch (96 px) and narrower than two: the next default stop is at 192 px
    let pa = Paragraph {
        runs: vec![Run::text("m".repeat(6), 20.0), Run::text("\tz", 20.0)],
        ..Default::default()
    };
    let l = lay(vec![pa], 800.0, 100.0);
    let m_width = line_w(&lay(vec![p(&"m".repeat(6), 20.0)], 800.0, 100.0).lines[0]);
    assert!(m_width > 96.0 && m_width < 192.0, "{m_width}");
    let z = l.lines[0]
        .frags
        .iter()
        .find(|f| f.text.ends_with('z'))
        .expect("z");
    assert!((z.x - 192.0).abs() < 0.01, "{}", z.x);
    // text shorter than an inch: the first stop
    let pa = Paragraph {
        runs: vec![Run::text("mm\tz", 20.0)],
        ..Default::default()
    };
    let l = lay(vec![pa], 800.0, 100.0);
    let z = l.lines[0]
        .frags
        .iter()
        .find(|f| f.text.ends_with('z'))
        .expect("z");
    assert!((z.x - 96.0).abs() < 0.01, "{}", z.x);
}

#[test]
fn a_line_fits_when_its_text_is_as_wide_as_the_box_whatever_follows_it_and_a_hundredth_over_still_fits(
) {
    let one = |s: &str| line_w(&lay(vec![p(s, 20.0)], 2000.0, 100.0).lines[0]);
    let w = one("ab cd");
    // exactly as wide as "ab cd": the space after "cd" does not count
    let l = lay(vec![p("ab cd ef", 20.0)], w + 0.001, 400.0);
    assert_eq!(texts(&l), vec!["ab cd", "ef"]);
    // a hundredth of a pixel too narrow still fits (rounding of the measure), a tenth does not
    let l = lay(vec![p("ab cd ef", 20.0)], w - 0.005, 400.0);
    assert_eq!(texts(&l), vec!["ab cd", "ef"]);
    let l = lay(vec![p("ab cd ef", 20.0)], w - 0.1, 400.0);
    assert_eq!(texts(&l), vec!["ab", "cd ef"]);
}

#[test]
fn the_pieces_of_a_word_broken_between_characters_keep_their_own_widths() {
    let one = |s: &str| line_w(&lay(vec![p(s, 20.0)], 2000.0, 100.0).lines[0]);
    let word = format!("{}{}", "i".repeat(8), "W".repeat(8));
    let (wi, ww) = (one("i"), one("W"));
    // a box that holds all the i and three W
    let width = 8.0 * wi + 3.0 * ww + 0.5;
    let l = lay(vec![p(&word, 20.0)], width, 400.0);
    assert_eq!(
        texts(&l)[0],
        format!("{}{}", "i".repeat(8), "W".repeat(3)),
        "{:?}",
        texts(&l)
    );
    assert_eq!(texts(&l).concat(), word);
}
