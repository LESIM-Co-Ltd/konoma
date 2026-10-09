//! Tests of the text layout (`text::layout`): wrapping, kinsoku, alignment, bullets, spacing,
//! autofit, anchoring, vertical text, numbering.

use super::*;
use crate::preview::office::slide_draw::model::*;

const PX_PT: f64 = 96.0 / 72.0;

fn body(paragraphs: Vec<Paragraph>) -> TextBody {
    TextBody {
        paragraphs,
        ..Default::default()
    }
}

fn p(text: &str, pt: f64) -> Paragraph {
    Paragraph {
        runs: vec![Run::text(text, pt)],
        ..Default::default()
    }
}

fn line_text(l: &LineBox) -> String {
    l.frags.iter().map(|f| f.text.as_str()).collect()
}

fn texts(l: &Layout) -> Vec<String> {
    l.lines.iter().map(line_text).collect()
}

fn line_w(l: &LineBox) -> f64 {
    match (l.frags.first(), l.frags.last()) {
        (Some(a), Some(b)) => b.x + b.width - a.x,
        _ => 0.0,
    }
}

fn lay(paragraphs: Vec<Paragraph>, w: f64, h: f64) -> Layout {
    layout(&body(paragraphs), w, h)
}

fn word_stream(l: &Layout) -> String {
    texts(l)
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[test]
fn single_line_fits() {
    let l = lay(vec![p("Hello world", 20.0)], 600.0, 200.0);
    assert_eq!(texts(&l), vec!["Hello world"]);
    assert_eq!(l.lines[0].frags[0].x, 0.0);
    assert!(l.lines[0].frags[0].width > 80.0);
    assert!(!l.truncated);
    assert_eq!(l.frame, Frame::Normal);
}

#[test]
fn latin_wraps_at_spaces_and_keeps_all_words() {
    let t = "The quick brown fox jumps over the lazy dog and keeps running far away";
    let l = lay(vec![p(t, 20.0)], 220.0, 400.0);
    assert!(l.lines.len() >= 4, "{:?}", texts(&l));
    for line in &l.lines {
        assert!(
            line_w(line) <= 220.0 + 0.5,
            "{} > 220: {:?}",
            line_w(line),
            line_text(line)
        );
        // wrapped lines neither start nor end with a space
        let s = line_text(line);
        assert_eq!(s, s.trim());
    }
    assert_eq!(word_stream(&l), t);
    // narrower box -> more lines, wider -> fewer
    let narrow = lay(vec![p(t, 20.0)], 120.0, 400.0).lines.len();
    let wide = lay(vec![p(t, 20.0)], 900.0, 400.0).lines.len();
    assert!(narrow > l.lines.len() && l.lines.len() > wide);
    assert_eq!(wide, 1);
}

#[test]
fn greedy_wrap_is_tight() {
    // the first word of the next line would not have fit on the previous one
    let t = "alpha beta gamma delta epsilon zeta eta theta iota kappa";
    let w = 260.0;
    let l = lay(vec![p(t, 18.0)], w, 400.0);
    for pair in l.lines.windows(2) {
        let next_first = line_text(&pair[1]).split(' ').next().unwrap().to_string();
        let merged = format!("{} {}", line_text(&pair[0]), next_first);
        let mw = crate::preview::office::slide_draw::fonts::measure(
            &pair[0].frags[0]
                .style
                .family
                .split(',')
                .map(|s| s.trim().trim_matches('\'').to_string())
                .collect::<Vec<_>>(),
            false,
            false,
            &merged,
            18.0 * PX_PT,
        );
        assert!(mw > w - 1.0, "{merged:?} would have fit ({mw} <= {w})");
    }
}

#[test]
fn long_word_breaks_by_characters() {
    let word = "Supercalifragilisticexpialidocious";
    let l = lay(vec![p(word, 20.0)], 90.0, 400.0);
    assert!(l.lines.len() >= 3, "{:?}", texts(&l));
    for line in &l.lines {
        assert!(
            line_w(line) <= 90.0 + 1.0,
            "{:?} {}",
            line_text(line),
            line_w(line)
        );
    }
    assert_eq!(texts(&l).concat(), word);
    // a single character wider than the box still gets a line (no infinite loop, no loss)
    let l = lay(vec![p("WWW", 40.0)], 5.0, 100.0);
    assert_eq!(texts(&l).concat(), "WWW");
    assert_eq!(l.lines.len(), 3);
}

#[test]
fn cjk_wraps_between_any_two_characters() {
    let t = "あ".repeat(20);
    let l = lay(vec![p(&t, 20.0)], 100.0, 400.0);
    let per = (100.0 / (20.0 * PX_PT)).floor() as usize;
    assert!(
        l.lines.len() >= 20 / per && l.lines.len() <= 20 / per + 2,
        "{:?}",
        texts(&l)
    );
    for line in &l.lines {
        assert!(line_w(line) <= 100.5);
    }
    assert_eq!(texts(&l).concat(), t);
    // mixed text: break between a Japanese character and a Latin word, no space needed
    let l = lay(vec![p("日本語English日本語English", 20.0)], 130.0, 400.0);
    assert!(l.lines.len() >= 2);
    assert_eq!(texts(&l).concat(), "日本語English日本語English");
}

#[test]
fn kinsoku_closing_punctuation_never_starts_a_line() {
    // find a width where the naive break would put 。 at the start of a line
    let size = 20.0;
    let cw = 20.0 * PX_PT;
    for n in 2..8 {
        let t = format!("{}。{}", "あ".repeat(n), "い".repeat(6));
        // exactly n ideographs fit in the width
        let l = lay(vec![p(&t, size)], cw * n as f64 + 1.0, 400.0);
        for line in &l.lines {
            let s = line_text(line);
            let first = s.chars().next().unwrap();
            assert!(
                !"。、）」』】".contains(first),
                "line starts with {first}: {:?}",
                texts(&l)
            );
        }
        assert_eq!(texts(&l).concat(), t);
    }
    // several closing characters in a row stay glued to what precedes them
    let t = "あああ」。、いい";
    let l = lay(vec![p(t, 20.0)], cw * 3.0 + 1.0, 400.0);
    for line in &l.lines {
        let first = line_text(line).chars().next().unwrap();
        assert!(!"」。、".contains(first), "{:?}", texts(&l));
    }
    // small kana and the long vowel mark do not start a line either
    let t = "あああゃ";
    let l = lay(vec![p(t, 20.0)], cw * 3.0 + 1.0, 400.0);
    assert!(!line_text(&l.lines[1]).starts_with('ゃ'), "{:?}", texts(&l));
}

#[test]
fn kinsoku_opening_brackets_never_end_a_line() {
    let cw = 20.0 * PX_PT;
    for n in 2..8 {
        let t = format!("{}「{}」", "あ".repeat(n), "い".repeat(4));
        let l = lay(vec![p(&t, 20.0)], cw * n as f64 + 1.0, 400.0);
        for line in &l.lines {
            let last = line_text(line).chars().last().unwrap();
            assert!(
                !"「（『【".contains(last),
                "line ends with {last}: {:?}",
                texts(&l)
            );
        }
        assert_eq!(texts(&l).concat(), t);
    }
    // ASCII brackets next to Japanese text follow the same rule
    let l = lay(vec![p("日本語日本語(括弧)日本語", 20.0)], cw * 7.0, 400.0);
    for line in &l.lines {
        assert!(!line_text(line).ends_with('('), "{:?}", texts(&l));
        assert!(!line_text(line).starts_with(')'), "{:?}", texts(&l));
    }
}

#[test]
fn forced_line_break_and_trailing_break() {
    let para = Paragraph {
        runs: vec![
            Run::text("first", 20.0),
            Run {
                kind: RunKind::LineBreak,
                ..Run::text("", 20.0)
            },
            Run::text("second", 20.0),
        ],
        ..Default::default()
    };
    let l = lay(vec![para], 600.0, 400.0);
    assert_eq!(texts(&l), vec!["first", "second"]);
    // a break at the very end leaves an empty last line
    let para = Paragraph {
        runs: vec![
            Run::text("x", 20.0),
            Run {
                kind: RunKind::LineBreak,
                ..Run::text("", 20.0)
            },
        ],
        ..Default::default()
    };
    let l = lay(vec![para], 600.0, 400.0);
    assert_eq!(l.lines.len(), 2);
    assert!(l.lines[1].frags.is_empty());
    assert!(l.lines[1].height > 20.0);
    // newline characters inside a run are breaks too
    let l = lay(vec![p("a\nb\u{b}c", 20.0)], 600.0, 400.0);
    assert_eq!(texts(&l), vec!["a", "b", "c"]);
}

#[test]
fn wrap_false_never_wraps_and_aligns_to_the_box() {
    let t = "this line is far too long for the little box but must stay on one line";
    let mut b = body(vec![p(t, 20.0)]);
    b.wrap = false;
    let l = layout(&b, 100.0, 200.0);
    assert_eq!(l.lines.len(), 1);
    assert!(line_w(&l.lines[0]) > 400.0);
    // centred text overflows both sides evenly
    b.paragraphs[0].align = Align::Center;
    let l = layout(&b, 100.0, 200.0);
    let f = &l.lines[0];
    let (left, right) = (f.frags[0].x, 100.0 - (f.frags[0].x + line_w(f)));
    assert!((left - right).abs() < 1.0 && left < 0.0, "{left} {right}");
    // right-aligned overflows to the left
    b.paragraphs[0].align = Align::Right;
    let l = layout(&b, 100.0, 200.0);
    let f = &l.lines[0];
    assert!(((f.frags[0].x + line_w(f)) - 100.0).abs() < 1.0);
}

#[test]
fn alignment_offsets() {
    let w = 400.0;
    let one = |a| {
        let mut pa = p("Align me", 20.0);
        pa.align = a;
        let l = lay(vec![pa], w, 100.0);
        let f = &l.lines[0].frags[0];
        (f.x, f.width)
    };
    let (lx, lw) = one(Align::Left);
    let (cx, cw) = one(Align::Center);
    let (rx, rw) = one(Align::Right);
    assert_eq!(lx, 0.0);
    assert!((cx - (w - cw) / 2.0).abs() < 0.01 && (cw - lw).abs() < 0.01);
    assert!((rx - (w - rw)).abs() < 0.01);
    // a single-line justified paragraph is left aligned
    assert_eq!(one(Align::Justify).0, 0.0);
}

#[test]
fn justify_fills_every_line_but_the_last() {
    let t = "Justified text spreads the gaps between the words so that every line except the final one touches both edges";
    let mut pa = p(t, 18.0);
    pa.align = Align::Justify;
    let w = 300.0;
    let l = lay(vec![pa], w, 400.0);
    assert!(l.lines.len() >= 4);
    for (i, line) in l.lines.iter().enumerate() {
        let end = line.frags.last().map(|f| f.x + f.width).unwrap();
        if i + 1 < l.lines.len() {
            assert!(
                (end - w).abs() < 0.6,
                "line {i} ends at {end}: {:?}",
                line_text(line)
            );
            assert_eq!(line.frags[0].x, 0.0);
        } else {
            assert!(end < w - 5.0, "last line must not stretch: {end}");
        }
    }
    assert_eq!(word_stream(&l), t);
}

#[test]
fn justify_cjk_spreads_between_characters_and_distributed_includes_the_last_line() {
    let mut pa = p(&"あ".repeat(11), 20.0);
    pa.align = Align::Justify;
    let cw = 20.0 * PX_PT;
    let l = lay(vec![pa], cw * 5.0 + 7.0, 300.0);
    let first = &l.lines[0];
    let end = first.frags.last().map(|f| f.x + f.width).unwrap();
    assert!((end - (cw * 5.0 + 7.0)).abs() < 0.6, "{end}");
    // distributed spreads the final line as well
    let mut pa = p("ab cd", 20.0);
    pa.align = Align::Distributed;
    let l = lay(vec![pa], 300.0, 100.0);
    let f = l.lines[0].frags.last().unwrap();
    assert!((f.x + f.width - 300.0).abs() < 0.6, "{}", f.x + f.width);
}

#[test]
fn margins_indent_and_hanging_bullets() {
    let t =
        "A bullet paragraph whose text is long enough to wrap onto at least two lines in this box";
    let mut pa = p(t, 18.0);
    pa.mar_l = 342_900.0; // 36 px
    pa.indent = -342_900.0;
    pa.bullet = Some(Bullet {
        kind: BulletKind::Char("•".into()),
        font: None,
        color: None,
        size: BulletSize::FollowText,
    });
    let l = lay(vec![pa], 300.0, 400.0);
    assert!(l.lines.len() >= 2);
    // text starts at the left margin on every line, the bullet hangs at x = 0
    for line in &l.lines {
        assert!((line.frags[0].x - 36.0).abs() < 0.01, "{}", line.frags[0].x);
        assert!(line_w(line) <= 300.0 - 36.0 + 0.5);
    }
    assert_eq!(l.bullets.len(), 1);
    match &l.bullets[0] {
        BulletDraw::Text(f) => {
            assert_eq!(f.text, "•");
            assert!(f.x.abs() < 0.01);
            assert!((f.y - l.lines[0].baseline).abs() < 0.01);
        }
        other => panic!("{other:?}"),
    }
    // margin without a bullet: first line indented
    let mut pa = p(t, 18.0);
    pa.mar_l = 190_500.0; // 20 px
    pa.indent = 190_500.0; // first line at 40 px
    let l = lay(vec![pa], 300.0, 400.0);
    assert!((l.lines[0].frags[0].x - 40.0).abs() < 0.01);
    assert!((l.lines[1].frags[0].x - 20.0).abs() < 0.01);
    // no bullets are drawn for an empty text? (a bullet on an empty paragraph is not shown)
}

#[test]
fn bullet_colour_font_and_size() {
    let mut pa = p("x", 20.0);
    pa.bullet = Some(Bullet {
        kind: BulletKind::Char("■".into()),
        font: None,
        color: Some(Rgba::rgb(1, 2, 3)),
        size: BulletSize::Pct(0.5),
    });
    pa.mar_l = 380_000.0;
    pa.indent = -380_000.0;
    let l = lay(vec![pa], 300.0, 100.0);
    let BulletDraw::Text(f) = &l.bullets[0] else {
        panic!()
    };
    assert_eq!(
        (f.style.color.r, f.style.color.g, f.style.color.b),
        (1, 2, 3)
    );
    assert!((f.style.size_px - 10.0 * PX_PT).abs() < 0.01);
    // points
    let mut pa = p("x", 20.0);
    pa.bullet = Some(Bullet {
        kind: BulletKind::Char("a".into()),
        font: None,
        color: None,
        size: BulletSize::Pts(30.0),
    });
    let l = lay(vec![pa], 300.0, 100.0);
    let BulletDraw::Text(f) = &l.bullets[0] else {
        panic!()
    };
    assert!((f.style.size_px - 30.0 * PX_PT).abs() < 0.01);
    // wingdings characters map to look-alikes
    let mut pa = p("x", 20.0);
    pa.bullet = Some(Bullet {
        kind: BulletKind::Char("§".into()),
        font: Some(FontSpec {
            latin: Some("Wingdings".into()),
            ..Default::default()
        }),
        color: None,
        size: BulletSize::FollowText,
    });
    let l = lay(vec![pa], 300.0, 100.0);
    let BulletDraw::Text(f) = &l.bullets[0] else {
        panic!()
    };
    assert_eq!(f.text, "▪");
    // picture bullet
    let mut pa = p("x", 20.0);
    pa.bullet = Some(Bullet {
        kind: BulletKind::Picture(ImageFill::stretch("k")),
        font: None,
        color: None,
        size: BulletSize::FollowText,
    });
    let l = lay(vec![pa], 300.0, 100.0);
    assert!(
        matches!(l.bullets[0], BulletDraw::Picture { size, .. } if (size - 20.0 * PX_PT).abs() < 0.01)
    );
}

#[test]
fn numbering_schemes() {
    assert_eq!(autonum_text("arabicPeriod", 3), "3.");
    assert_eq!(autonum_text("arabicParenR", 3), "3)");
    assert_eq!(autonum_text("arabicParenBoth", 3), "(3)");
    assert_eq!(autonum_text("arabicPlain", 3), "3");
    assert_eq!(autonum_text("alphaLcPeriod", 1), "a.");
    assert_eq!(autonum_text("alphaUcPeriod", 2), "B.");
    assert_eq!(autonum_text("alphaLcPeriod", 26), "z.");
    assert_eq!(autonum_text("alphaLcPeriod", 27), "aa.");
    assert_eq!(autonum_text("alphaUcParenR", 28), "BB)");
    assert_eq!(autonum_text("romanLcPeriod", 4), "iv.");
    assert_eq!(autonum_text("romanUcPeriod", 14), "XIV.");
    assert_eq!(autonum_text("romanUcPeriod", 1994), "MCMXCIV.");
    assert_eq!(autonum_text("romanLcParenBoth", 9), "(ix)");
    assert_eq!(autonum_text("circleNumDbPlain", 3), "③");
    assert_eq!(autonum_text("circleNumDbPlain", 21), "21.");
    assert_eq!(autonum_text("circleNumWdBlackPlain", 1), "❶");
    // unknown schemes fall back to arabicPeriod
    assert_eq!(autonum_text("thaiNumPeriod", 5), "5.");
    assert_eq!(autonum_text("whatever", 7), "7.");
    // degenerate numbers do not panic
    assert_eq!(autonum_text("romanLcPeriod", 0), "0.");
    assert_eq!(autonum_text("alphaLcPeriod", 0), "0.");
    assert_eq!(
        autonum_text("romanLcPeriod", u32::MAX),
        format!("{}.", u32::MAX)
    );
    assert_eq!(autonum_text("circleNumDbPlain", 0), "0.");
}

fn numbered(level: u8, text: &str, scheme: &str, start: u32) -> Paragraph {
    Paragraph {
        level,
        bullet: Some(Bullet {
            kind: BulletKind::AutoNum {
                scheme: scheme.into(),
                start,
            },
            font: None,
            color: None,
            size: BulletSize::FollowText,
        }),
        mar_l: 342_900.0,
        indent: -342_900.0,
        ..p(text, 18.0)
    }
}

fn bullet_texts(l: &Layout) -> Vec<String> {
    l.bullets
        .iter()
        .map(|b| match b {
            BulletDraw::Text(f) => f.text.clone(),
            BulletDraw::Picture { .. } => "<pic>".into(),
        })
        .collect()
}

#[test]
fn numbering_continues_and_restarts() {
    let l = lay(
        vec![
            numbered(0, "a", "arabicPeriod", 1),
            numbered(0, "b", "arabicPeriod", 1),
            numbered(1, "c", "alphaLcPeriod", 1),
            numbered(1, "d", "alphaLcPeriod", 1),
            numbered(0, "e", "arabicPeriod", 1),
            p("plain interrupts", 18.0),
            numbered(0, "f", "arabicPeriod", 1),
            numbered(0, "g", "romanLcPeriod", 1),
            numbered(0, "h", "arabicPeriod", 5),
        ],
        500.0,
        900.0,
    );
    assert_eq!(
        bullet_texts(&l),
        vec!["1.", "2.", "a.", "b.", "3.", "1.", "i.", "5."]
    );
}

#[test]
fn autofit_scales_fonts_and_spacing() {
    let t = "Scaled text";
    let plain = lay(vec![p(t, 40.0), p(t, 40.0)], 800.0, 400.0);
    let mut b = body(vec![p(t, 40.0), p(t, 40.0)]);
    b.autofit = AutoFit::Normal {
        font_scale: 0.5,
        ln_spc_reduction: 0.0,
    };
    let half = layout(&b, 800.0, 400.0);
    assert!(
        (half.lines[0].frags[0].style.size_px * 2.0 - plain.lines[0].frags[0].style.size_px).abs()
            < 0.01
    );
    assert!((half.lines[0].frags[0].width * 2.0 - plain.lines[0].frags[0].width).abs() < 3.0);
    assert!((half.content_h * 2.0 - plain.content_h).abs() < 0.5);
    // line spacing reduction applies to percentage spacing
    let mut pa = p(t, 20.0);
    pa.line_spacing = Spacing::Pct(1.0);
    let mut b = body(vec![pa.clone(), pa]);
    b.autofit = AutoFit::Normal {
        font_scale: 1.0,
        ln_spc_reduction: 0.2,
    };
    let reduced = layout(&b, 800.0, 400.0);
    let pitch = reduced.lines[1].baseline - reduced.lines[0].baseline;
    assert!((pitch - 0.8 * 1.2 * 20.0 * PX_PT).abs() < 0.01, "{pitch}");
    // a font scale above 100 % is ignored, junk is ignored
    for fs in [2.0, f64::NAN, -1.0, 0.0] {
        let mut b = body(vec![p(t, 20.0)]);
        b.autofit = AutoFit::Normal {
            font_scale: fs,
            ln_spc_reduction: f64::NAN,
        };
        let l = layout(&b, 800.0, 400.0);
        assert!(
            (l.lines[0].frags[0].style.size_px - 20.0 * PX_PT).abs() < 0.01,
            "{fs}"
        );
    }
    // shape autofit leaves the layout alone
    let mut b = body(vec![p(t, 20.0)]);
    b.autofit = AutoFit::Shape;
    assert!((layout(&b, 800.0, 400.0).lines[0].frags[0].style.size_px - 20.0 * PX_PT).abs() < 0.01);
}

#[test]
fn line_spacing_percent_and_points() {
    let pitch = |ls: Spacing| {
        let mut pa = p(
            "one two three four five six seven eight nine ten eleven twelve",
            20.0,
        );
        pa.line_spacing = ls;
        let l = lay(vec![pa], 200.0, 600.0);
        assert!(l.lines.len() >= 3);
        l.lines[1].baseline - l.lines[0].baseline
    };
    let single = 1.2 * 20.0 * PX_PT;
    assert!((pitch(Spacing::Pct(1.0)) - single).abs() < 0.01);
    assert!((pitch(Spacing::Pct(2.0)) - 2.0 * single).abs() < 0.01);
    assert!((pitch(Spacing::Pct(0.5)) - 0.5 * single).abs() < 0.01);
    assert!((pitch(Spacing::Pts(30.0)) - 40.0).abs() < 0.01);
    // exact spacing smaller than the font is allowed
    assert!((pitch(Spacing::Pts(10.0)) - 10.0 * PX_PT).abs() < 0.01);
    // junk falls back to single spacing / is clamped
    assert!((pitch(Spacing::Pts(f64::NAN)) - single).abs() < 0.01);
    assert!(pitch(Spacing::Pct(f64::NAN)) >= 0.0);
}

#[test]
fn percent_spacing_puts_the_extra_above_the_baseline() {
    let first_baseline = |ls| {
        let mut pa = p("x", 20.0);
        pa.line_spacing = ls;
        lay(vec![pa], 200.0, 200.0).lines[0].baseline
    };
    let b1 = first_baseline(Spacing::Pct(1.0));
    let b2 = first_baseline(Spacing::Pct(2.0));
    assert!((b1 - 0.93 * 20.0 * PX_PT).abs() < 0.01);
    assert!(b2 > b1 + 15.0);
}

#[test]
fn paragraph_spacing() {
    let mut a = p("first", 20.0);
    a.spc_after = Spacing::Pts(12.0);
    let mut b = p("second", 20.0);
    b.spc_before = Spacing::Pts(6.0);
    let l = lay(vec![a, b], 500.0, 300.0);
    let single = 1.2 * 20.0 * PX_PT;
    let gap = l.lines[1].baseline - l.lines[0].baseline - single;
    assert!((gap - 18.0 * PX_PT).abs() < 0.01, "{gap}");
    // space before the first paragraph is ignored
    let mut a = p("first", 20.0);
    a.spc_before = Spacing::Pts(40.0);
    let l = lay(vec![a], 500.0, 300.0);
    assert!((l.lines[0].top).abs() < 0.01);
    // percentage spacing follows the font size
    let mut a = p("a", 20.0);
    a.spc_after = Spacing::Pct(0.5);
    let l = lay(vec![a, p("b", 20.0)], 500.0, 300.0);
    let gap = l.lines[1].baseline - l.lines[0].baseline - single;
    assert!((gap - 0.5 * single).abs() < 0.01, "{gap}");
}

#[test]
fn anchors_and_insets_in_layout_space() {
    let h = 300.0;
    let single = 1.2 * 20.0 * PX_PT;
    let at = |a| {
        let mut b = body(vec![p("x", 20.0)]);
        b.anchor = a;
        layout(&b, 200.0, h).lines[0].top
    };
    assert!((at(Anchor::Top)).abs() < 0.01);
    assert!((at(Anchor::Middle) - (h - single) / 2.0).abs() < 0.01);
    assert!((at(Anchor::Bottom) - (h - single)).abs() < 0.01);
    // overflow: text taller than the box with a bottom anchor starts above the box
    let many: Vec<_> = (0..20).map(|_| p("line", 20.0)).collect();
    let mut b = body(many);
    b.anchor = Anchor::Bottom;
    let l = layout(&b, 200.0, 100.0);
    assert!(l.lines[0].top < 0.0);
    let last = l.lines.last().unwrap();
    assert!((last.top + last.height - 100.0).abs() < 0.01);
}

#[test]
fn anchor_centre_centres_the_block() {
    let mut b = body(vec![p("short", 20.0), p("a longer second line", 20.0)]);
    b.anchor_ctr = true;
    let l = layout(&b, 600.0, 100.0);
    let widest = l.lines.iter().map(line_w).fold(0.0, f64::max);
    let x0 = l
        .lines
        .iter()
        .map(|l| l.frags[0].x)
        .fold(f64::INFINITY, f64::min);
    assert!((x0 - (600.0 - widest) / 2.0).abs() < 0.5, "{x0}");
    // both lines keep their own left edge
    assert!((l.lines[0].frags[0].x - l.lines[1].frags[0].x).abs() < 0.01);
}

#[test]
fn empty_paragraphs_keep_their_height() {
    let mut empty = Paragraph {
        end_size_pt: 30.0,
        ..Default::default()
    };
    empty.runs.clear();
    let l = lay(vec![p("a", 20.0), empty, p("b", 20.0)], 300.0, 400.0);
    assert_eq!(l.lines.len(), 3);
    assert!(l.lines[1].frags.is_empty());
    assert!((l.lines[1].height - 1.2 * 30.0 * PX_PT).abs() < 0.01);
    let gap = l.lines[2].top - l.lines[0].top;
    assert!((gap - (1.2 * 20.0 * PX_PT + 1.2 * 30.0 * PX_PT)).abs() < 0.01);
    // a paragraph of only spaces/empty text also keeps its height
    let l = lay(vec![p("", 20.0), p("b", 20.0)], 300.0, 400.0);
    assert_eq!(l.lines.len(), 2);
    assert!(l.lines[1].top > 20.0);
    // no paragraphs at all
    let l = lay(vec![], 300.0, 400.0);
    assert!(l.lines.is_empty() && l.content_h == 0.0);
}

#[test]
fn mixed_sizes_share_a_line_height_and_baseline() {
    let pa = Paragraph {
        runs: vec![
            Run::text("small ", 12.0),
            Run::text("BIG", 36.0),
            Run::text(" small", 12.0),
        ],
        ..Default::default()
    };
    let l = lay(vec![pa], 600.0, 200.0);
    assert_eq!(l.lines.len(), 1);
    let line = &l.lines[0];
    assert!((line.height - 1.2 * 36.0 * PX_PT).abs() < 0.01);
    let ys: Vec<f64> = line.frags.iter().map(|f| f.y).collect();
    assert!(ys.iter().all(|y| (*y - ys[0]).abs() < 0.001));
    assert_eq!(line.frags.len(), 3);
    // fragments follow each other without gaps
    for pair in line.frags.windows(2) {
        assert!((pair[0].x + pair[0].width - pair[1].x).abs() < 0.001);
    }
}

#[test]
fn super_and_subscripts() {
    let pa = Paragraph {
        runs: vec![
            Run::text("x", 24.0),
            Run {
                baseline_pct: 30.0,
                ..Run::text("2", 24.0)
            },
            Run {
                baseline_pct: -25.0,
                ..Run::text("i", 24.0)
            },
        ],
        ..Default::default()
    };
    let l = lay(vec![pa], 300.0, 100.0);
    let f = &l.lines[0].frags;
    assert_eq!(f.len(), 3);
    let base = 24.0 * PX_PT;
    assert!((f[1].style.size_px - base * 2.0 / 3.0).abs() < 0.01);
    assert!(f[1].y < f[0].y - 0.29 * base && f[1].y > f[0].y - 0.31 * base);
    assert!(f[2].y > f[0].y + 0.24 * base && f[2].y < f[0].y + 0.26 * base);
}

#[test]
fn caps_and_spacing() {
    let l = lay(
        vec![Paragraph {
            runs: vec![Run {
                caps: Caps::All,
                ..Run::text("shout", 20.0)
            }],
            ..Default::default()
        }],
        400.0,
        100.0,
    );
    assert_eq!(texts(&l), vec!["SHOUT"]);
    // small caps: capitals at 80 % for the originally lower-case letters
    let l = lay(
        vec![Paragraph {
            runs: vec![Run {
                caps: Caps::Small,
                ..Run::text("Small", 20.0)
            }],
            ..Default::default()
        }],
        400.0,
        100.0,
    );
    assert_eq!(texts(&l), vec!["SMALL"]);
    let sizes: Vec<f64> = l.lines[0].frags.iter().map(|f| f.style.size_px).collect();
    assert!(sizes.len() == 2 && sizes[0] > sizes[1], "{sizes:?}");
    // character spacing widens the text
    let w = |sp: f64| {
        let l = lay(
            vec![Paragraph {
                runs: vec![Run {
                    spacing_pt: sp,
                    ..Run::text("abcdef", 20.0)
                }],
                ..Default::default()
            }],
            600.0,
            100.0,
        );
        l.lines[0].frags[0].width
    };
    assert!((w(3.0) - w(0.0) - 6.0 * 3.0 * PX_PT).abs() < 0.01);
    assert!(w(-1.0) < w(0.0));
}

#[test]
fn script_split_uses_the_right_font_per_character() {
    let pa = Paragraph {
        runs: vec![Run {
            font: FontSpec {
                latin: Some("Calibri".into()),
                east_asian: Some("游ゴシック".into()),
                ..Default::default()
            },
            ..Run::text("abc日本語def", 20.0)
        }],
        ..Default::default()
    };
    let l = lay(vec![pa], 600.0, 100.0);
    let f = &l.lines[0].frags;
    assert!(f.len() >= 3, "{f:?}");
    assert!(f[0].style.family.contains("Calibri"));
    assert!(f
        .iter()
        .any(|x| x.style.family.contains("Hiragino Sans") && x.text.contains('日')));
    assert!(f.last().unwrap().style.family.contains("Calibri"));
    assert_eq!(texts(&l), vec!["abc日本語def"]);
}

#[test]
fn bold_and_italic_are_measured_with_their_own_face() {
    let w = |bold, italic| {
        let l = lay(
            vec![Paragraph {
                runs: vec![Run {
                    bold,
                    italic,
                    font: FontSpec {
                        latin: Some("Arial".into()),
                        ..Default::default()
                    },
                    ..Run::text("Measure this wide text", 24.0)
                }],
                ..Default::default()
            }],
            900.0,
            100.0,
        );
        l.lines[0].frags[0].width
    };
    assert!(w(true, false) > w(false, false) + 1.0);
    assert!(w(false, false) > 100.0);
}

#[test]
fn vertical_frames() {
    for (v, f) in [
        (Vert::Vert, Frame::Rot90),
        (Vert::Vert270, Frame::Rot270),
        (Vert::Horz, Frame::Normal),
        (Vert::EaVert, Frame::Normal),
    ] {
        let mut b = body(vec![p(
            "vertical text that needs a few lines to be shown",
            20.0,
        )]);
        b.vert = v;
        let l = layout(&b, 100.0, 400.0);
        assert_eq!(l.frame, f, "{v:?}");
        if matches!(v, Vert::Vert | Vert::Vert270) {
            // lines run along the rectangle's height
            assert_eq!((l.frame_w, l.frame_h), (400.0, 100.0));
            for line in &l.lines {
                assert!(line_w(line) <= 400.0 + 0.5);
            }
            assert!(l.lines.len() <= 3);
        }
    }
}

#[test]
fn east_asian_vertical_text_stacks_columns_right_to_left() {
    let mut b = body(vec![p("縦書きの文章がここに入ります。二行目です。", 20.0)]);
    b.vert = Vert::EaVert;
    let l = layout(&b, 200.0, 120.0);
    assert!(l.lines.len() >= 3, "{}", l.lines.len());
    // columns move leftwards
    let xs: Vec<f64> = l.lines.iter().map(|c| c.frags[0].x).collect();
    assert!(xs.windows(2).all(|w| w[1] < w[0]), "{xs:?}");
    // glyphs run top to bottom inside a column and stay inside the box
    for col in &l.lines {
        let ys: Vec<f64> = col.frags.iter().map(|f| f.y).collect();
        assert!(ys.windows(2).all(|w| w[1] > w[0]), "{ys:?}");
        assert!(*ys.last().unwrap() <= 123.0);
        for f in &col.frags {
            assert!(f.x >= 0.0 && f.x <= 200.0);
            assert!(!f.rot90);
        }
    }
    // Latin words are rotated
    let mut b = body(vec![p("縦ABC横", 20.0)]);
    b.vert = Vert::EaVert;
    let l = layout(&b, 100.0, 300.0);
    let f = &l.lines[0].frags;
    assert!(f.iter().any(|x| x.rot90 && x.text == "ABC"), "{f:?}");
    assert!(f.iter().any(|x| !x.rot90 && x.text == "縦"));
}

#[test]
fn columns_flow_top_to_bottom() {
    let many: Vec<_> = (0..10).map(|i| p(&format!("line {i}"), 20.0)).collect();
    let mut b = body(many);
    b.columns = 2;
    let l = layout(&b, 400.0, 130.0);
    let (c0, c1): (Vec<_>, Vec<_>) = l.lines.iter().partition(|ln| ln.frags[0].x < 200.0);
    assert!(!c0.is_empty() && !c1.is_empty());
    assert!(c0.iter().all(|ln| ln.top + ln.height <= 130.0 + 0.01));
    assert!((c1[0].top).abs() < 0.01);
    assert_eq!(c0.len() + c1.len(), 10);
    // text is laid out in the column width
    let mut b = body(vec![p(
        "a few words that need wrapping in a narrow column of text here",
        20.0,
    )]);
    b.columns = 2;
    let l = layout(&b, 400.0, 1000.0);
    assert!(l.lines.iter().all(|ln| line_w(ln) <= 200.5));
}

#[test]
fn right_to_left_paragraphs_mirror() {
    let mut pa = p("one two", 20.0);
    pa.rtl = true;
    pa.align = Align::Right; // logical start side = left in the mirrored frame
    let l = lay(vec![pa.clone()], 400.0, 100.0);
    let fr = &l.lines[0].frags;
    // the line is flush with the right edge (the left-to-right frame's "start" mirrored)
    assert_eq!(fr.len(), 1);
    assert!(
        (fr[0].x + fr[0].width - 400.0).abs() < 0.01,
        "{}",
        fr[0].x + fr[0].width
    );
    // two styles: the first logical fragment is the right-most
    let mut two = pa.clone();
    two.runs = vec![
        Run::text("one ", 20.0),
        Run {
            bold: true,
            ..Run::text("two", 20.0)
        },
    ];
    let l2 = lay(vec![two], 400.0, 100.0);
    let f = &l2.lines[0].frags;
    assert!(f[0].x > f[1].x, "{} {}", f[0].x, f[1].x);
    // mirrored margins: hanging bullet sits at the right edge
    pa.align = Align::Left;
    pa.mar_l = 342_900.0;
    pa.indent = -342_900.0;
    pa.bullet = Some(Bullet {
        kind: BulletKind::Char("•".into()),
        font: None,
        color: None,
        size: BulletSize::FollowText,
    });
    let l = lay(vec![pa], 400.0, 100.0);
    let BulletDraw::Text(b) = &l.bullets[0] else {
        panic!()
    };
    assert!(b.x + b.width > 390.0, "{}", b.x + b.width);
}

#[test]
fn text_limit_truncates() {
    let big = "x".repeat(MAX_TEXT_CHARS + 5000);
    let l = lay(vec![p(&big, 10.0)], 400.0, 400.0);
    assert!(l.truncated);
    let total: usize = texts(&l).iter().map(|s| s.chars().count()).sum();
    assert!(total <= MAX_TEXT_CHARS);
    // line cap
    let many: Vec<_> = (0..(MAX_LINES / 2 + 10)).map(|_| p("a b", 10.0)).collect();
    let _ = lay(many, 400.0, 400.0);
}

#[test]
fn hostile_sizes_and_widths() {
    for (w, h) in [
        (0.0, 0.0),
        (-5.0, -5.0),
        (f64::NAN, f64::NAN),
        (f64::INFINITY, 10.0),
        (1e300, 1e300),
        (1.0, 1.0),
    ] {
        let l = lay(vec![p("some text 日本語 here", 20.0)], w, h);
        for line in &l.lines {
            for f in &line.frags {
                assert!(
                    f.x.is_finite() && f.y.is_finite() && f.width.is_finite(),
                    "{w} {h}"
                );
            }
        }
    }
    for s in [f64::NAN, f64::INFINITY, -4.0, 0.0, 1e12, 1e-9] {
        let l = lay(vec![p("size", s)], 300.0, 100.0);
        for line in &l.lines {
            for f in &line.frags {
                assert!(
                    f.style.size_px.is_finite()
                        && f.style.size_px > 0.0
                        && f.style.size_px <= 5000.0
                );
                assert!(f.x.is_finite() && f.y.is_finite());
            }
        }
    }
}

#[test]
fn trailing_and_leading_spaces() {
    // spaces at the wrap point disappear; spaces at the very start of a paragraph stay
    let l = lay(vec![p("  lead", 20.0)], 400.0, 100.0);
    assert!(l.lines[0].frags[0].x > 0.0 || l.lines[0].frags[0].text.starts_with("  "));
    assert_eq!(line_text(&l.lines[0]), "  lead");
    let l = lay(
        vec![p("word     more words here to wrap", 20.0)],
        120.0,
        300.0,
    );
    for line in l.lines.iter().skip(1) {
        assert!(!line_text(line).starts_with(' '));
    }
    // tabs are spaces
    let l = lay(vec![p("a\tb", 20.0)], 400.0, 100.0);
    assert_eq!(texts(&l), vec!["a b"]);
}

#[test]
fn highlight_and_decorations_are_carried_on_fragments() {
    let pa = Paragraph {
        runs: vec![Run {
            highlight: Some(Rgba::rgb(255, 255, 0)),
            underline: Underline::Double,
            strike: Strike::Double,
            fill: Fill::Gradient(Gradient::linear(
                0.0,
                vec![(0.0, Rgba::rgb(9, 8, 7)), (1.0, Rgba::WHITE)],
            )),
            ..Run::text("deco", 20.0)
        }],
        ..Default::default()
    };
    let l = lay(vec![pa], 400.0, 100.0);
    let s = &l.lines[0].frags[0].style;
    assert_eq!(s.highlight, Some(Rgba::rgb(255, 255, 0)));
    assert_eq!(s.underline, Underline::Double);
    assert_eq!(s.strike, Strike::Double);
    // a gradient text fill is drawn with its first colour
    assert_eq!((s.color.r, s.color.g, s.color.b), (9, 8, 7));
}

#[test]
fn control_characters_are_dropped() {
    let l = lay(vec![p("a\u{0}\u{1}b\u{200b}c\r", 20.0)], 400.0, 100.0);
    assert_eq!(texts(&l), vec!["abc"]);
}
