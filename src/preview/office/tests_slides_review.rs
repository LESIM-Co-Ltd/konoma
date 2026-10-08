//! Tests added by the review of the slide previews (PR #37): the reading order rule (wider band,
//! underlays, shapes off the slide), `mc:AlternateContent` per ECMA-376 part 3, embedded objects,
//! separate bullet lists, slide-number fields, placeholders that name an `idx` the layout lacks, and
//! the budgets of the relationships and of the parts around the slides.

use super::docx::pptx::*;
use super::docx::*;
use super::tests::{deflated, tmp, write};
use super::tests_docx::tiny_png;
use super::tests_odp as od;
use super::tests_pptx as px;
use super::tests_pptx::I;
use super::*;

fn lines(m: &str) -> Vec<&str> {
    m.lines().filter(|l| !l.is_empty()).collect()
}

/// A presentation whose entries are edited before it is zipped.
fn load_edited(
    p: &px::Px,
    opts: &DocOptions,
    edit: impl FnOnce(&mut Vec<(String, Vec<u8>)>),
) -> Result<Document, OfficeError> {
    let mut e = p.entries();
    edit(&mut e);
    let refs: Vec<(&str, &[u8])> = e.iter().map(|(n, b)| (n.as_str(), b.as_slice())).collect();
    let dir = tmp("pptxrv");
    let path = write(&dir, "t.pptx", &deflated(&refs));
    load_presentation(&path, opts)
}

fn edit_part(e: &mut [(String, Vec<u8>)], name: &str, f: impl Fn(String) -> String) {
    for (n, b) in e.iter_mut() {
        if n == name {
            *b = f(String::from_utf8(b.clone()).unwrap()).into_bytes();
        }
    }
}

// ---------------------------------------------------------------------------------------------
// reading order, end to end
// ---------------------------------------------------------------------------------------------

#[test]
fn a_comparison_with_equal_gaps_is_read_side_by_side() {
    // Heading 0.2 inch above its text, columns 0.2 inch apart: the tie goes to columns.
    let g = I / 5;
    let s = [
        px::tb(0, 0, 4 * I, I, &px::pa("Left heading")),
        px::tb(4 * I + g, 0, 4 * I, I, &px::pa("Right heading")),
        px::tb(0, I + g, 4 * I, 3 * I, &px::pa("Left text")),
        px::tb(4 * I + g, I + g, 4 * I, 3 * I, &px::pa("Right text")),
    ]
    .concat();
    assert_eq!(
        lines(&px::md1(&s)),
        vec![
            "## Slide 1",
            "Left heading",
            "Left text",
            "Right heading",
            "Right text"
        ]
    );
}

#[test]
fn three_cards_are_read_card_by_card() {
    let g = I / 4;
    let mut s = String::new();
    for c in 0..3 {
        s += &px::tb(c * (3 * I + g), 0, 3 * I, I, &px::pa(&format!("T{c}")));
    }
    for c in 0..3 {
        s += &px::tb(
            c * (3 * I + g),
            I + g,
            3 * I,
            2 * I,
            &px::pa(&format!("B{c}")),
        );
    }
    assert_eq!(
        lines(&px::md1(&s)),
        vec!["## Slide 1", "T0", "B0", "T1", "B1", "T2", "B2"]
    );
}

#[test]
fn label_and_value_rows_are_read_row_by_row() {
    let mut s = String::new();
    for r in 0..3 {
        let y = r * (I + 2 * I / 5);
        s += &px::tb(0, y, 2 * I, I, &px::pa(&format!("label{r}")));
        s += &px::tb(2 * I + I / 5, y, 4 * I, I, &px::pa(&format!("value{r}")));
    }
    assert_eq!(
        lines(&px::md1(&s)),
        vec![
            "## Slide 1",
            "label0",
            "value0",
            "label1",
            "value1",
            "label2",
            "value2"
        ]
    );
}

#[test]
fn a_full_slide_background_picture_does_not_interleave_two_columns() {
    let mut s = px::pic("rId5", "background", Some((0, 0, 9 * I, 7 * I)));
    let cells: Vec<(i64, i64, String)> = (0..3)
        .flat_map(|r| {
            [
                (I, I + r * 2 * I, format!("L{}", r + 1)),
                (6 * I, I + r * 2 * I, format!("R{}", r + 1)),
            ]
        })
        .collect();
    for (x, y, t) in cells {
        s += &px::tb(x, y, 2 * I, I, &px::pa(&t));
    }
    let p = px::Px::new(vec![px::sl(&s).rel(
        "rId5",
        "image",
        "../media/bg.png",
        false,
    )])
    .media("bg.png", tiny_png(1));
    let m = px::doc(&p).markdown;
    let l = lines(&m);
    assert!(l[1].starts_with("![background](office-img://"), "{m}");
    assert_eq!(&l[2..], &["L1", "L2", "L3", "R1", "R2", "R3"], "{m}");
}

#[test]
fn a_text_box_on_a_picture_is_read_after_the_picture() {
    let s = [
        px::tb(I, I, 2 * I, I, &px::pa("caption")),
        px::pic("rId5", "photo", Some((0, 0, 6 * I, 4 * I))),
    ]
    .concat();
    let p = px::Px::new(vec![px::sl(&s).rel(
        "rId5",
        "image",
        "../media/f.png",
        false,
    )])
    .media("f.png", tiny_png(1));
    let m = px::doc(&p).markdown;
    let l = lines(&m);
    assert!(l[1].starts_with("![photo]"), "{m}");
    assert_eq!(l[2], "caption");
}

#[test]
fn shapes_off_the_slide_are_kept_and_read_after_the_ones_on_it() {
    // (The slide is 9144000 x 6858000: 10 x 7.5 inches.)
    let s = [
        px::tb(-50 * I, 0, I, I, &px::pa("far left")),
        px::tb_nopos(&px::pa("no position")),
        px::tb(0, 100 * I, I, I, &px::pa("far below")),
        px::tb(0, 5 * I, I, I, &px::pa("on slide low")),
        px::tb(0, 0, I, I, &px::pa("on slide high")),
    ]
    .concat();
    assert_eq!(
        lines(&px::md1(&s)),
        vec![
            "## Slide 1",
            "on slide high",
            "on slide low",
            "far left",
            "far below",
            "no position"
        ]
    );
}

#[test]
fn a_deck_without_a_slide_size_reads_off_slide_shapes_by_position() {
    let s = [
        px::tb(-50 * I, 0, I, I, &px::pa("far left")),
        px::tb(0, 5 * I, I, I, &px::pa("low")),
    ]
    .concat();
    let d = load_edited(
        &px::Px::new(vec![px::sl(&s)]),
        &DocOptions::default(),
        |e| {
            edit_part(e, "ppt/presentation.xml", |s| {
                s.replace(r#"<p:sldSz cx="9144000" cy="6858000"/>"#, "")
            })
        },
    )
    .unwrap();
    assert_eq!(lines(&d.markdown), vec!["## Slide 1", "far left", "low"]);
}

#[test]
fn odp_shapes_off_the_page_are_read_last() {
    let layout = r#"<style:page-layout style:name="PM1"><style:page-layout-properties fo:page-width="28cm" fo:page-height="15.75cm"/></style:page-layout>"#;
    let master = r#"<style:master-page style:name="Default" style:page-layout-name="PM1"/>"#;
    let s = od::tf(-200.0, 1.0, 5.0, 1.0, "far left")
        + &od::tf(1.0, 12.0, 5.0, 1.0, "low")
        + &od::tf(1.0, 1.0, 5.0, 1.0, "high");
    let o = od::Op::new(&od::page_with("dp1", "Default", &s))
        .styles_auto(layout)
        .master(master);
    assert_eq!(
        od::doc(&o).markdown,
        "## Slide 1\n\nhigh\n\nlow\n\nfar left"
    );
    // Without the page layout the size is unknown: position order.
    let o = od::Op::new(&od::page_with("dp1", "Default", &s)).master(master);
    assert_eq!(
        od::doc(&o).markdown,
        "## Slide 1\n\nfar left\n\nhigh\n\nlow"
    );
}

#[test]
fn odp_a_background_frame_goes_first_and_columns_stay_columns() {
    let mut s = od::tf(0.0, 0.0, 28.0, 15.0, "backdrop");
    for r in 0..3 {
        s += &od::tf(1.0, 1.0 + r as f64 * 4.0, 8.0, 2.0, &format!("L{}", r + 1));
        s += &od::tf(15.0, 1.0 + r as f64 * 4.0, 8.0, 2.0, &format!("R{}", r + 1));
    }
    assert_eq!(
        od::body1(&s),
        "backdrop\n\nL1\n\nL2\n\nL3\n\nR1\n\nR2\n\nR3"
    );
}

// ---------------------------------------------------------------------------------------------
// mc:AlternateContent
// ---------------------------------------------------------------------------------------------

fn ac(requires: &str, choice: &str, fallback: &str) -> String {
    format!(
        r#"<mc:AlternateContent><mc:Choice Requires="{requires}">{choice}</mc:Choice><mc:Fallback>{fallback}</mc:Fallback></mc:AlternateContent>"#
    )
}

#[test]
fn ink_falls_back_to_its_picture() {
    // PowerPoint writes ink as a content part (p14) with a picture as the fallback.
    let s = ac(
        "p14",
        r#"<p:contentPart r:id="rIdInk"/>"#,
        &px::pic("rId5", "ink drawing", Some((0, 0, 2 * I, 2 * I))),
    );
    let p = px::Px::new(vec![px::sl(&s).rel(
        "rId5",
        "image",
        "../media/ink.png",
        false,
    )])
    .media("ink.png", tiny_png(3));
    let m = px::doc(&p).markdown;
    assert!(m.contains("![ink drawing](office-img://"), "{m}");
}

#[test]
fn an_unknown_requires_takes_the_fallback_and_a_known_one_the_choice() {
    let one = |r: &str| {
        px::md1(&ac(
            r,
            &px::tb(0, 0, I, I, &px::pa("choice")),
            &px::tb(0, 0, I, I, &px::pa("fallback")),
        ))
    };
    assert_eq!(one("a14"), "## Slide 1\n\nchoice");
    assert_eq!(one("p14"), "## Slide 1\n\nfallback");
    assert_eq!(one("xyz"), "## Slide 1\n\nfallback");
    // Every namespace must be understood.
    assert_eq!(one("a14 p14"), "## Slide 1\n\nfallback");
    assert_eq!(one("a14 a14"), "## Slide 1\n\nchoice");
    // No `Requires` requires nothing.
    let none = format!(
        r#"<mc:AlternateContent><mc:Choice>{}</mc:Choice><mc:Fallback>{}</mc:Fallback></mc:AlternateContent>"#,
        px::tb(0, 0, I, I, &px::pa("choice")),
        px::tb(0, 0, I, I, &px::pa("fallback"))
    );
    assert_eq!(px::md1(&none), "## Slide 1\n\nchoice");
}

#[test]
fn a_choice_nobody_can_read_and_no_fallback_is_nothing() {
    let s = format!(
        r#"<mc:AlternateContent><mc:Choice Requires="p14">{}</mc:Choice></mc:AlternateContent>"#,
        px::tb(0, 0, I, I, &px::pa("lost"))
    );
    assert_eq!(px::md1(&s), "## Slide 1");
}

#[test]
fn a_second_choice_is_taken_when_the_first_is_not_readable() {
    let s = format!(
        r#"<mc:AlternateContent><mc:Choice Requires="p14">{}</mc:Choice><mc:Choice Requires="a14">{}</mc:Choice><mc:Fallback>{}</mc:Fallback></mc:AlternateContent>"#,
        px::tb(0, 0, I, I, &px::pa("first")),
        px::tb(0, 0, I, I, &px::pa("second")),
        px::tb(0, 0, I, I, &px::pa("fallback"))
    );
    assert_eq!(px::md1(&s), "## Slide 1\n\nsecond");
}

#[test]
fn inline_alternate_content_follows_the_same_rule() {
    let tx = |req: &str| {
        px::paras(&format!(
            r#"{}{}"#,
            px::ru("", "x "),
            ac(
                req,
                "<a:r><a:t>choice</a:t></a:r>",
                "<a:r><a:t>fallback</a:t></a:r>"
            )
        ))
    };
    assert_eq!(px::body1(&px::tb(0, 0, I, I, &tx("a14"))), "x choice");
    assert_eq!(px::body1(&px::tb(0, 0, I, I, &tx("p14"))), "x fallback");
}

// ---------------------------------------------------------------------------------------------
// embedded objects
// ---------------------------------------------------------------------------------------------

fn ole_frame(inner: &str) -> String {
    format!(
        r#"<p:graphicFrame><p:nvGraphicFramePr><p:cNvPr id="90" name="Obj"/><p:cNvGraphicFramePr/><p:nvPr/></p:nvGraphicFramePr><p:xfrm><a:off x="0" y="0"/><a:ext cx="{I}" cy="{I}"/></p:xfrm><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/presentationml/2006/ole">{inner}</a:graphicData></a:graphic></p:graphicFrame>"#
    )
}

#[test]
fn an_embedded_object_without_a_picture_leaves_a_mark() {
    let o = ole_frame(
        r#"<p:oleObj progId="Word.Document.12" name="Doc" r:id="rId9"><p:embed/></p:oleObj>"#,
    );
    assert_eq!(
        px::md1(&o),
        "## Slide 1\n\n\\[object: Word.Document.12]",
        "progId first"
    );
    let named = ole_frame(r#"<p:oleObj name="Worksheet" r:id="rId9"><p:embed/></p:oleObj>"#);
    assert_eq!(px::md1(&named), "## Slide 1\n\n\\[object: Worksheet]");
    let bare = ole_frame(r#"<p:oleObj r:id="rId9"><p:embed/></p:oleObj>"#);
    assert_eq!(px::md1(&bare), "## Slide 1\n\n\\[object]");
    // Markdown in the name cannot do anything.
    let evil = ole_frame(r#"<p:oleObj progId="x](http://e.example/)[y"><p:embed/></p:oleObj>"#);
    let m = px::md1(&evil);
    // (Every `[` is escaped, so no link can open.)
    assert!(!m.replace("\\[", "").contains('['), "{m}");
}

#[test]
fn an_object_in_alternate_content_with_a_fallback_picture_shows_the_picture() {
    let inner = String::from(
        r#"<mc:AlternateContent><mc:Choice Requires="v"><p:oleObj progId="Excel.Sheet.12" r:id="rId9"><p:embed/></p:oleObj></mc:Choice><mc:Fallback><p:oleObj progId="Excel.Sheet.12" r:id="rId9"><p:embed/><p:pic><p:nvPicPr><p:cNvPr id="1" name="x"/><p:cNvPicPr/><p:nvPr/></p:nvPicPr><p:blipFill><a:blip r:embed="rId5"/></p:blipFill></p:pic></p:oleObj></mc:Fallback></mc:AlternateContent>"#,
    );
    let p = px::Px::new(vec![px::sl(&ole_frame(&inner)).rel(
        "rId5",
        "image",
        "../media/o.png",
        false,
    )])
    .media("o.png", tiny_png(5));
    let m = px::doc(&p).markdown;
    assert!(m.contains("office-img://"), "{m}");
    assert!(!m.contains("object"), "{m}");
}

// ---------------------------------------------------------------------------------------------
// lists, fields, tables
// ---------------------------------------------------------------------------------------------

#[test]
fn bullets_of_separate_shapes_are_separate_lists() {
    let l = |x: i64, y: i64, a: &str, b: &str| {
        px::body((x, y, 3 * I, 2 * I), &(px::pa(a) + &px::pa(b)))
    };
    let s = l(0, 2 * I, "l1", "l2") + &l(5 * I, 2 * I, "r1", "r2");
    assert_eq!(
        px::body1(&s),
        "- l1\n- l2\n\n<!-- -->\n\n- r1\n- r2",
        "two shapes, two lists"
    );
    // One shape stays one list even with a plain paragraph in the way being absent.
    let one = px::body((0, 2 * I, 3 * I, 3 * I), &(px::pa("a") + &px::pa("b")));
    assert_eq!(px::body1(&one), "- a\n- b");
    // A paragraph between two lists already separates them: no comment is added.
    let mid =
        l(0, I, "a", "b") + &px::tb(0, 4 * I, I, I, &px::pa("between")) + &l(0, 5 * I, "c", "d");
    let m = px::body1(&mid);
    assert!(!m.contains("<!-- -->"), "{m}");
}

#[test]
fn lettered_lists_of_separate_shapes_are_separate_paragraphs() {
    let l = |x: i64, a: &str, b: &str| {
        let item = |t: &str| px::pp("", r#"<a:buAutoNum type="alphaLcPeriod"/>"#, t);
        px::tb(x, 2 * I, 3 * I, 2 * I, &(item(a) + &item(b)))
    };
    let s = l(0, "x", "y") + &l(5 * I, "z", "w");
    // Inside one shape the lines of a list stay together; two shapes are two paragraphs.
    assert_eq!(px::body1(&s), "a. x  \nb. y\n\na. z  \nb. w");
}

#[test]
fn odp_bullets_of_separate_frames_are_separate_lists() {
    let list = |a: &str, b: &str| {
        format!(
            r#"<text:list text:style-name="L1"><text:list-item>{}</text:list-item><text:list-item>{}</text:list-item></text:list>"#,
            od::tp(a),
            od::tp(b)
        )
    };
    let s = od::fr("outline", 1.0, 1.0, 8.0, 4.0, &od::tbx(&list("l1", "l2")))
        + &od::fr("outline", 12.0, 1.0, 8.0, 4.0, &od::tbx(&list("r1", "r2")));
    let d = od::doc(&od::Op::new(&od::page(&s)).auto(od::L1));
    assert_eq!(
        d.markdown,
        "## Slide 1\n\n- l1\n- l2\n\n<!-- -->\n\n- r1\n- r2"
    );
}

fn slide_no_text() -> String {
    px::paras(&format!(
        r#"{}<a:fld id="{{B6F15528-21DE-4FAA-801E-634DDDAF4B2B}}" type="slidenum"><a:rPr lang="en-US"/><a:t>‹#›</a:t></a:fld>"#,
        px::ru("", "page ")
    ))
}

#[test]
fn a_slide_number_field_shows_the_number_of_its_slide() {
    let t = px::tb(0, 0, 2 * I, I, &slide_no_text());
    let p = px::Px::new(vec![px::sl(&t), px::sl(&t).hidden(), px::sl(&t)]);
    let d = px::doc(&p);
    for n in 1..=3 {
        assert!(d.markdown.contains(&format!("page {n}")), "{}", d.markdown);
    }
    assert!(!d.markdown.contains('‹'), "{}", d.markdown);
}

#[test]
fn the_first_slide_number_of_the_presentation_is_honoured() {
    let t = px::tb(0, 0, 2 * I, I, &slide_no_text());
    let p = px::Px::new(vec![px::sl(&t), px::sl(&t)]);
    let d = load_edited(&p, &DocOptions::default(), |e| {
        edit_part(e, "ppt/presentation.xml", |s| {
            s.replace("<p:presentation ", r#"<p:presentation firstSlideNum="5" "#)
        })
    })
    .unwrap();
    assert!(d.markdown.contains("page 5"), "{}", d.markdown);
    assert!(d.markdown.contains("page 6"), "{}", d.markdown);
}

#[test]
fn odp_a_page_number_field_shows_the_number_of_its_slide() {
    let t = String::from(
        "<text:p>page <text:page-number text:select-page=\"current\">&lt;number&gt;</text:page-number></text:p>"
    );
    let s = od::fr("", 1.0, 1.0, 5.0, 1.0, &od::tbx(&t));
    let o = od::Op::new(&(od::page(&s) + &od::page(&s)));
    let d = od::doc(&o);
    assert_eq!(d.markdown, "## Slide 1\n\npage 1\n\n## Slide 2\n\npage 2");
}

#[test]
fn a_table_without_first_row_is_still_written_with_a_header_line_like_word_tables() {
    // Markdown tables need a header row: as in Word tables, the first row is it whatever
    // `firstRow` says.
    let t = px::table(
        (0, 0, 4 * I, I),
        &(px::tr(&["a", "b"]) + &px::tr(&["1", "2"])),
    );
    assert_eq!(px::body1(&t), "| a | b |\n| --- | --- |\n| 1 | 2 |");
    let flagged = t.replace("<a:tblPr/>", r#"<a:tblPr firstRow="1"/>"#);
    assert_eq!(px::body1(&flagged), px::body1(&t));
}

// ---------------------------------------------------------------------------------------------
// placeholders
// ---------------------------------------------------------------------------------------------

#[test]
fn a_placeholder_with_an_idx_the_layout_lacks_inherits_no_position() {
    // The layout has a body at the left (idx 1). The slide's body names idx 3: nothing to inherit,
    // so it is "no position" and comes last, after the text box on the right.
    let layout = px::ph(r#"type="body" idx="1""#, Some((0, I, 4 * I, 4 * I)), "");
    let s = [
        px::ph(r#"type="body" idx="3""#, None, &px::pa("orphan")),
        px::tb(5 * I, I, 4 * I, 4 * I, &px::pa("right")),
    ]
    .concat();
    let p = px::Px::new(vec![px::sl(&s)]).layout(0, &layout);
    assert_eq!(
        lines(&px::doc(&p).markdown),
        vec!["## Slide 1", "right", "- orphan"]
    );
    // The same placeholder with idx 1 does inherit the left position and comes first.
    let s = [
        px::ph(r#"type="body" idx="1""#, None, &px::pa("laid out")),
        px::tb(5 * I, I, 4 * I, 4 * I, &px::pa("right")),
    ]
    .concat();
    let p = px::Px::new(vec![px::sl(&s)]).layout(0, &layout);
    assert_eq!(
        lines(&px::doc(&p).markdown),
        vec!["## Slide 1", "- laid out", "right"]
    );
}

#[test]
fn a_placeholder_with_no_idx_finds_the_layouts_by_its_type() {
    let layout = px::ph(r#"type="body" idx="1""#, Some((0, I, 4 * I, 4 * I)), "");
    let s = [
        px::ph(r#"type="body""#, None, &px::pa("by type")),
        px::tb(5 * I, I, 4 * I, 4 * I, &px::pa("right")),
    ]
    .concat();
    let p = px::Px::new(vec![px::sl(&s)]).layout(0, &layout);
    assert_eq!(
        lines(&px::doc(&p).markdown),
        vec!["## Slide 1", "- by type", "right"]
    );
}

// ---------------------------------------------------------------------------------------------
// hostile input
// ---------------------------------------------------------------------------------------------

#[test]
fn a_rotated_shape_with_extreme_numbers_does_not_overflow() {
    let m = i64::MAX;
    let spp = format!(
        r#"<a:xfrm rot="2700000"><a:off x="{m}" y="{m}"/><a:ext cx="{m}" cy="{m}"/></a:xfrm>"#
    );
    let s = [
        px::sp_raw(5, "", &spp, &px::pa("huge")),
        px::tb(0, 0, I, I, &px::pa("normal")),
    ]
    .concat();
    // (Tests run with overflow checks: an overflow here would be a panic and a Corrupt result.)
    let d = px::doc(&px::Px::new(vec![px::sl(&s)]));
    assert!(d.markdown.contains("huge") && d.markdown.contains("normal"));
    let neg = spp.replace(&m.to_string(), &i64::MIN.to_string());
    let s = px::sp_raw(5, "", &neg, &px::pa("tiny"));
    assert!(px::md1(&s).contains("tiny"));
}

fn nest(depth: usize, leaf: &str) -> String {
    let mut s = leaf.to_string();
    for _ in 0..depth {
        s = px::group((0, 0, 100, 100), (0, 0, 100, 100), &s);
    }
    s
}

#[test]
fn groups_deeper_than_followed_set_truncated() {
    let leaf = px::tb(0, 0, 10, 10, &px::pa("deep"));
    // 20 levels are followed.
    let d = px::doc(&px::Px::new(vec![px::sl(&nest(20, &leaf))]));
    assert!(d.markdown.contains("deep"), "{}", d.markdown);
    assert!(!d.truncated);
    // One more is not, and the reader says so.
    let d = px::doc(&px::Px::new(vec![px::sl(&nest(21, &leaf))]));
    assert!(!d.markdown.contains("deep"), "{}", d.markdown);
    assert!(d.truncated);
}

fn rels_hog(n: usize) -> px::Sl {
    let mut s = px::sl(&px::tb(0, 0, I, I, &px::pa("hello")));
    for i in 0..n {
        s = s.rel(
            &format!("rIdX{i}"),
            "hyperlink",
            &format!("https://e.example/{i}"),
            true,
        );
    }
    s
}

#[test]
fn a_slide_listed_again_after_itself_is_parsed_once() {
    // E_bigrels: a 185 KB file with 1,000 references to one slide whose relationships part is
    // megabytes. Each reference used to read, parse and copy it again (31 seconds).
    let p = px::Px::new(vec![rels_hog(30_000)]);
    let t = std::time::Instant::now();
    let d = load_edited(&p, &DocOptions::default(), |e| {
        edit_part(e, "ppt/presentation.xml", |s| {
            s.replace(
                r#"<p:sldId id="256" r:id="rIdS0"/>"#,
                &r#"<p:sldId id="256" r:id="rIdS0"/>"#.repeat(300),
            )
        })
    })
    .unwrap();
    assert_eq!(d.slides.len(), 300);
    assert!(d.markdown.contains("## Slide 300"));
    assert!(t.elapsed().as_secs() < 10, "{:?}", t.elapsed());
}

#[test]
fn relationships_count_against_the_read_budget_and_say_so() {
    // Two slides with big relationship parts, listed alternately: every reference reads its part
    // again, which the total read budget now limits (and reports).
    let p = px::Px::new(vec![
        rels_hog(20_000).file("a.xml"),
        rels_hog(20_000).file("b.xml"),
    ]);
    let opts = DocOptions {
        max_pptx_read_total: 20 * 1024 * 1024,
        ..DocOptions::default()
    };
    let t = std::time::Instant::now();
    let d = load_edited(&p, &opts, |e| {
        edit_part(e, "ppt/presentation.xml", |s| {
            s.replace(
                r#"<p:sldId id="256" r:id="rIdS0"/><p:sldId id="257" r:id="rIdS1"/>"#,
                &r#"<p:sldId id="256" r:id="rIdS0"/><p:sldId id="257" r:id="rIdS1"/>"#.repeat(200),
            )
        })
    })
    .unwrap();
    assert!(d.truncated);
    assert_eq!(d.slides.len(), 400, "every slide keeps its place");
    px::check_headings(&d);
    assert!(t.elapsed().as_secs() < 20, "{:?}", t.elapsed());
}

#[test]
fn the_presentation_part_is_read_under_the_part_cap_like_a_slide() {
    let p = px::Px::new(vec![px::sl(&px::tb(0, 0, I, I, &px::pa("x")))]);
    let pad = "<!--".to_string() + &"c".repeat(30_000) + "-->";
    let edit = |e: &mut Vec<(String, Vec<u8>)>| {
        edit_part(e, "ppt/presentation.xml", |s| {
            s.replacen("<p:sldIdLst>", &format!("{pad}<p:sldIdLst>"), 1)
        })
    };
    // Under the cap: read.
    let ok = load_edited(&p, &DocOptions::default(), edit).unwrap();
    assert_eq!(ok.slides.len(), 1);
    // Over the cap of a part (here 20,000 bytes): not read in.
    let small = DocOptions {
        max_slide_part_bytes: 20_000,
        ..DocOptions::default()
    };
    let r = load_edited(&p, &small, edit);
    assert!(r.is_err(), "{:?}", r.map(|d| d.markdown));
}
