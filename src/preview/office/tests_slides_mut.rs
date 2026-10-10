//! Tests that kill the mutants the mutation review of the slide previews (PR #37) left alive, and
//! those of a second round aimed at the code the review fixes added: bullets and numbering, the
//! placeholders a shape inherits, rotation and group transforms (told apart through the reading
//! order), the budgets and their exact edges, the relationships that must stay inside the package,
//! `mc:AlternateContent` per namespace, and the OpenDocument counterparts (`has_content`, master
//! pages, page layouts, page-number fields).
//!
//! Every test names the behaviour it pins; a limit is tested **at** the limit (everything shown, no
//! `truncated`) and **one past** it (cut, `truncated`).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use super::docx::pptx::*;
use super::docx::*;
use super::tests::{deflated, tmp, write};
use super::tests_docx as dx;
use super::tests_odp as od;
use super::tests_pptx as px;
use super::tests_pptx::I;
use super::*;

// ---------------------------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------------------------

/// The non-empty lines, without their indentation.
fn trimmed(m: &str) -> Vec<String> {
    m.lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect()
}

/// `words` that occur in `md`, in the order of their first occurrence.
fn order(md: &str, words: &[&str]) -> Vec<String> {
    let mut v: Vec<(usize, &str)> = words
        .iter()
        .filter_map(|w| md.find(w).map(|i| (i, *w)))
        .collect();
    v.sort();
    v.into_iter().map(|(_, w)| w.to_string()).collect()
}

fn load_entries(e: &[(String, Vec<u8>)], opts: &DocOptions) -> Result<Document, OfficeError> {
    let refs: Vec<(&str, &[u8])> = e.iter().map(|(n, b)| (n.as_str(), b.as_slice())).collect();
    let dir = tmp("pptxmut");
    let path = write(&dir, "t.pptx", &deflated(&refs));
    load_presentation(&path, opts)
}

/// A presentation whose entries are edited before it is zipped.
fn load_edited(
    p: &px::Px,
    opts: &DocOptions,
    edit: impl FnOnce(&mut Vec<(String, Vec<u8>)>),
) -> Result<Document, OfficeError> {
    let mut e = p.entries();
    edit(&mut e);
    load_entries(&e, opts)
}

fn edit_part(e: &mut [(String, Vec<u8>)], name: &str, f: impl Fn(String) -> String) {
    let mut seen = false;
    for (n, b) in e.iter_mut() {
        if n == name {
            seen = true;
            *b = f(String::from_utf8(b.clone()).unwrap()).into_bytes();
        }
    }
    assert!(seen, "no part {name}");
}

fn part_len(e: &[(String, Vec<u8>)], name: &str) -> u64 {
    e.iter().find(|(n, _)| n == name).unwrap().1.len() as u64
}

fn opts_with(f: impl FnOnce(&mut DocOptions)) -> DocOptions {
    let mut o = DocOptions::default();
    f(&mut o);
    o
}

/// A text box with a list style of its own.
fn tb_lst(lst: &str, tx: &str) -> String {
    format!(
        r#"<p:sp><p:nvSpPr><p:cNvPr id="91" name="L"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr>{}</p:spPr><p:txBody><a:bodyPr/><a:lstStyle>{lst}</a:lstStyle>{tx}</p:txBody></p:sp>"#,
        px::xf(0, 0, I, I)
    )
}

/// A paragraph at a level with a bullet element (`<a:buChar/>` ..).
fn bu_p(lvl: u8, bu: &str, t: &str) -> String {
    format!(r#"<a:p><a:pPr lvl="{lvl}">{bu}</a:pPr><a:r><a:t>{t}</a:t></a:r></a:p>"#)
}

const BU_CHAR: &str = r#"<a:buChar char="x"/>"#;

fn auto(ty: &str, start: Option<&str>) -> String {
    match start {
        Some(s) => format!(r#"<a:buAutoNum type="{ty}" startAt="{s}"/>"#),
        None => format!(r#"<a:buAutoNum type="{ty}"/>"#),
    }
}

fn one_box(tx: &str) -> String {
    px::body1(&px::tb(0, 0, I, I, tx))
}

// ---------------------------------------------------------------------------------------------
// bullets and numbering
// ---------------------------------------------------------------------------------------------

#[test]
fn a_picture_bullet_is_a_bullet() {
    let tx = bu_p(0, r#"<a:buBlip><a:blip r:embed="rId9"/></a:buBlip>"#, "pic");
    assert_eq!(one_box(&tx), "- pic");
}

#[test]
fn an_auto_number_with_no_type_counts_in_arabic_with_a_period() {
    let tx = bu_p(0, "<a:buAutoNum/>", "one") + &bu_p(0, "<a:buAutoNum/>", "two");
    assert_eq!(one_box(&tx), "1. one\n2. two");
}

#[test]
fn the_number_a_list_starts_at_is_read_and_kept_in_range() {
    for (start, want) in [
        ("12", "12. x"),
        ("7", "7. x"),
        ("0", "1. x"),
        ("-3", "1. x"),
        ("abc", "1. x"),
        ("", "1. x"),
        ("999999", "999999. x"),
        ("1000000", "999999. x"),
        ("4294967295", "999999. x"),
        (" 9 ", "9. x"),
    ] {
        let tx = bu_p(0, &auto("arabicPeriod", Some(start)), "x");
        assert_eq!(one_box(&tx), want, "startAt={start:?}");
    }
    // Without a `startAt` the count starts at 1.
    assert_eq!(one_box(&bu_p(0, &auto("arabicPeriod", None), "x")), "1. x");
}

#[test]
fn every_number_scheme_is_labelled_the_way_it_is_written() {
    for (ty, start, want) in [
        ("romanLcPeriod", "4", "iv. x"),
        ("romanUcPeriod", "4", "IV. x"),
        ("alphaLcPeriod", "3", "c. x"),
        ("alphaUcPeriod", "3", "C. x"),
        ("alphaLcParenR", "2", "b) x"),
        ("alphaUcParenR", "2", "B) x"),
        ("romanLcParenBoth", "3", "(iii) x"),
        ("romanUcParenR", "3", "III) x"),
        ("arabicParenBoth", "2", "(2) x"),
        ("arabicParenR", "2", "2\\) x"),
        ("arabicPlain", "2", "2 x"),
        ("circleNumDbPlain", "2", "\u{2461} x"),
        // A scheme that is none of the above counts in arabic; Markdown numbers it itself when
        // the label is `N.`: the schemes of other scripts with a period.
        ("ea1JpnChsDbPeriod", "3", "3. x"),
        ("hindiNumPeriod", "7", "7. x"),
        // ... and keeps the label as text when it is not (`N)`).
        ("thaiNumParenR", "2", "2\\) x"),
        ("hindiNumParenBoth", "2", "(2) x"),
    ] {
        let tx = bu_p(0, &auto(ty, Some(start)), "x");
        assert_eq!(one_box(&tx), want, "{ty}");
    }
}

#[test]
fn a_circled_number_never_gets_a_period() {
    // (Every scheme of the standard that starts with `circleNum` ends in `Plain`; the reader holds
    // to the prefix, so a scheme of this kind written otherwise has no period either.)
    let tx = bu_p(0, &auto("circleNumWdBlackPlain", Some("3")), "x");
    assert_eq!(one_box(&tx), "\u{2462} x");
    let tx = bu_p(0, &auto("circleNumSomethingNew", Some("3")), "x");
    assert_eq!(one_box(&tx), "\u{2462} x");
}

#[test]
fn all_nine_levels_of_a_list_style_are_read_and_a_level_is_clamped_to_them() {
    // Only the ninth level has a bullet. A paragraph at level 8 (the ninth) and one at an
    // out-of-range level 9 (clamped to it) are bullets; the level-0 and level-10 styles are no
    // levels at all.
    let lst = r#"<a:lvl9pPr><a:buChar char="x"/></a:lvl9pPr><a:lvl0pPr><a:buChar char="y"/></a:lvl0pPr><a:lvl10pPr><a:buChar char="z"/></a:lvl10pPr>"#;
    let tx = [px::pl(8, "eight"), px::pl(9, "nine"), px::pl(0, "zero")].concat();
    let m = px::body1(&tb_lst(lst, &tx));
    assert_eq!(trimmed(&m), vec!["- eight", "- nine", "zero"], "{m:?}");
}

#[test]
fn a_bullet_after_a_number_restarts_the_count_of_its_level() {
    let n = |t: &str| bu_p(0, &auto("arabicPeriod", None), t);
    let tx = [n("a"), bu_p(0, BU_CHAR, "b"), n("c"), n("d")].concat();
    let m = one_box(&tx);
    let t = trimmed(&m);
    let at = |w: &str| {
        t.iter()
            .find(|l| l.ends_with(w))
            .unwrap_or_else(|| panic!("{w} in {m:?}"))
            .clone()
    };
    assert_eq!(at("a"), "1. a", "{m:?}");
    assert_eq!(at("b"), "- b", "{m:?}");
    assert_eq!(
        at("c"),
        "1. c",
        "the count starts again after a bullet: {m:?}"
    );
    assert_eq!(at("d"), "2. d", "{m:?}");
}

#[test]
fn a_paragraph_that_shows_nothing_does_not_break_the_count() {
    let n = |t: &str| bu_p(0, &auto("arabicPeriod", None), t);
    let tx = [n("a"), n("   "), n("b")].concat();
    assert_eq!(one_box(&tx), "1. a\n2. b");
}

#[test]
fn separate_text_boxes_are_separate_lists_also_when_they_hold_one_item() {
    let s = [
        px::tb(0, 0, 4 * I, I, &bu_p(0, BU_CHAR, "first")),
        px::tb(0, 2 * I, 4 * I, I, &bu_p(0, BU_CHAR, "second")),
    ]
    .concat();
    assert_eq!(px::body1(&s), "- first\n\n<!-- -->\n\n- second");
}

#[test]
fn a_list_of_a_text_box_and_the_one_of_smartart_are_separate_lists() {
    let s = [
        px::tb(0, 0, 4 * I, I, &bu_p(0, BU_CHAR, "above")),
        px::smart_frame("rIdDm", (0, 3 * I, 4 * I, 3 * I)),
    ]
    .concat();
    let p = px::Px::new(vec![px::sl(&s).rel(
        "rIdDm",
        "diagramData",
        "../diagrams/data1.xml",
        false,
    )])
    .part("ppt/diagrams/data1.xml", &px::dm(&["below"], None));
    assert_eq!(
        px::doc(&p).markdown,
        "## Slide 1\n\n- above\n\n<!-- -->\n\n- below"
    );
    // Two diagrams in a row are two lists too.
    let s = [
        px::smart_frame("rIdDm", (0, 0, 4 * I, 2 * I)),
        px::smart_frame("rIdDm2", (0, 3 * I, 4 * I, 2 * I)),
    ]
    .concat();
    let p = px::Px::new(vec![px::sl(&s)
        .rel("rIdDm", "diagramData", "../diagrams/data1.xml", false)
        .rel("rIdDm2", "diagramData", "../diagrams/data2.xml", false)])
    .part("ppt/diagrams/data1.xml", &px::dm(&["one"], None))
    .part("ppt/diagrams/data2.xml", &px::dm(&["two"], None));
    assert_eq!(
        px::doc(&p).markdown,
        "## Slide 1\n\n- one\n\n<!-- -->\n\n- two"
    );
}

#[test]
fn a_text_box_gets_the_other_style_of_the_master_and_a_placeholder_its_own() {
    let mut p = px::Px::new(vec![px::sl(&px::tb(0, 0, I, I, &px::pa("plain")))]);
    p.tx_styles = r#"<p:txStyles><p:titleStyle/><p:bodyStyle/><p:otherStyle><a:lvl1pPr><a:buChar char="x"/></a:lvl1pPr></p:otherStyle></p:txStyles>"#.into();
    assert_eq!(px::doc(&p).markdown, "## Slide 1\n\n- plain");
}

#[test]
fn a_header_placeholder_is_not_content() {
    let s = [
        px::ph(r#"type="hdr""#, Some((0, 0, I, I)), &px::pa("header text")),
        px::ph(r#"type="ftr""#, Some((0, I, I, I)), &px::pa("footer text")),
        px::ph(r#"type="dt""#, Some((0, 2 * I, I, I)), &px::pa("date text")),
        px::ph(r#"type="sldNum""#, Some((0, 3 * I, I, I)), &px::pa("12")),
    ]
    .concat();
    assert_eq!(px::md1(&s), "## Slide 1");
}

// ---------------------------------------------------------------------------------------------
// run formatting, tables, pictures, frames
// ---------------------------------------------------------------------------------------------

#[test]
fn true_and_one_both_switch_a_style_on_and_a_double_strike_is_a_strike() {
    let tx = px::paras(
        &[
            px::ru(r#"b="true""#, "bold"),
            px::ru("", " "),
            px::ru(r#"i="true""#, "italic"),
            px::ru("", " "),
            px::ru(r#"strike="dblStrike""#, "gone"),
            px::ru("", " "),
            px::ru(r#"b=" 1 ""#, "padded"),
        ]
        .concat(),
    );
    assert_eq!(
        px::body1(&px::tb(0, 0, I, I, &tx)),
        "**bold** *italic* ~~gone~~ **padded**"
    );
}

fn merged_table(h: &str, v: &str) -> String {
    let cell = |attr: &str, t: &str| {
        format!(
            r#"<a:tc {attr}><a:txBody><a:bodyPr/><a:lstStyle/>{}</a:txBody><a:tcPr/></a:tc>"#,
            px::pa(t)
        )
    };
    let rows = format!(
        r#"<a:tr h="1">{}{}</a:tr><a:tr h="1">{}{}</a:tr>"#,
        cell("", "head1"),
        cell(h, "hmerged"),
        cell(v, "vmerged"),
        cell("", "tail")
    );
    px::table((0, 0, 4 * I, 2 * I), &rows)
}

#[test]
fn the_cells_a_merge_covers_are_empty() {
    for (h, v) in [
        (r#"hMerge="1""#, r#"vMerge="1""#),
        (r#"hMerge="true""#, r#"vMerge="true""#),
    ] {
        let m = px::md1(&merged_table(h, v));
        assert!(!m.contains("hmerged"), "{h}: {m}");
        assert!(!m.contains("vmerged"), "{v}: {m}");
        assert!(m.contains("head1") && m.contains("tail"), "{m}");
    }
    // A cell that merges nothing keeps its text.
    let m = px::md1(&merged_table("", r#"vMerge="0""#));
    assert!(m.contains("hmerged") && m.contains("vmerged"), "{m}");
}

#[test]
fn a_table_is_cut_at_its_cell_budget_exactly() {
    let opts = opts_with(|o| o.max_table_cells = 4);
    let rows = |n: usize| {
        let cells: Vec<String> = (0..n).map(|i| format!("c{i}")).collect();
        let refs: Vec<&str> = cells.iter().map(String::as_str).collect();
        // Rows of two cells.
        refs.chunks(2).map(px::tr).collect::<Vec<_>>().concat()
    };
    let at = px::load_px(
        &px::Px::new(vec![px::sl(&px::table((0, 0, 4 * I, 2 * I), &rows(4)))]),
        &opts,
    )
    .unwrap();
    assert!(!at.truncated);
    for i in 0..4 {
        assert!(at.markdown.contains(&format!("c{i}")), "{}", at.markdown);
    }
    let over = px::load_px(
        &px::Px::new(vec![px::sl(&px::table((0, 0, 4 * I, 2 * I), &rows(5)))]),
        &opts,
    )
    .unwrap();
    assert!(over.truncated);
    assert!(over.markdown.contains("c3"), "{}", over.markdown);
    assert!(!over.markdown.contains("c4"), "{}", over.markdown);
}

fn pic_with(descr: Option<&str>, title: Option<&str>) -> String {
    let d = descr
        .map(|d| format!(r#" descr="{d}""#))
        .unwrap_or_default();
    let t = title
        .map(|t| format!(r#" title="{t}""#))
        .unwrap_or_default();
    format!(
        r#"<p:pic><p:nvPicPr><p:cNvPr id="40" name="Pic"{d}{t}/><p:cNvPicPr/><p:nvPr/></p:nvPicPr><p:blipFill><a:blip r:embed="rIdNone"/></p:blipFill><p:spPr>{}</p:spPr></p:pic>"#,
        px::xf(0, 0, I, I)
    )
}

#[test]
fn a_picture_is_described_by_its_description_before_its_title() {
    let m = px::md1(&pic_with(Some("the description"), Some("the title")));
    assert!(m.contains("the description"), "{m}");
    assert!(!m.contains("the title"), "{m}");
    // Without a description (or with a blank one) the title is it.
    let m = px::md1(&pic_with(None, Some("the title")));
    assert!(m.contains("the title"), "{m}");
    let m = px::md1(&pic_with(Some("   "), Some("the title")));
    assert!(m.contains("the title"), "{m}");
}

fn unknown_frame(alt: &str) -> String {
    format!(
        r#"<p:graphicFrame><p:nvGraphicFramePr><p:cNvPr id="90" name="U" descr="{alt}"/><p:cNvGraphicFramePr/><p:nvPr/></p:nvGraphicFramePr><p:xfrm><a:off x="0" y="0"/><a:ext cx="{I}" cy="{I}"/></p:xfrm><a:graphic><a:graphicData uri="urn:example:unknown"><x:thing xmlns:x="urn:example:x"/></a:graphicData></a:graphic></p:graphicFrame>"#
    )
}

#[test]
fn a_frame_nobody_knows_is_a_placeholder_only_when_it_has_a_description() {
    assert_eq!(px::md1(&unknown_frame("")), "## Slide 1");
    assert_eq!(px::md1(&unknown_frame("  ")), "## Slide 1");
    assert_eq!(
        px::md1(&unknown_frame("Some widget")),
        "## Slide 1\n\n\\[Some widget]"
    );
}

#[test]
fn a_paragraph_that_is_only_a_display_formula_is_shown() {
    let tx = px::paras(
        r#"<mc:AlternateContent><mc:Choice Requires="a14"><a14:m><m:oMathPara><m:oMath><m:r><m:t>xy</m:t></m:r></m:oMath></m:oMathPara></a14:m></mc:Choice><mc:Fallback><a:r><a:t>fb</a:t></a:r></mc:Fallback></mc:AlternateContent>"#,
    );
    let m = px::body1(&px::tb(0, 0, I, I, &tx));
    assert!(m.starts_with("$$") && m.contains("xy"), "{m:?}");
    assert!(!m.contains("fb"), "{m:?}");
}

#[test]
fn a_blank_title_is_not_the_heading_and_a_real_one_after_it_is() {
    let s = [px::title("   "), px::title("Real")].concat();
    let d = px::doc(&px::Px::new(vec![px::sl(&s)]));
    assert_eq!(d.markdown, "## Slide 1: Real");
    assert_eq!(d.slides[0].title, "Real");
}

#[test]
fn a_title_and_an_object_name_are_cut_at_their_length() {
    let long = "t".repeat(201);
    let d = px::doc(&px::Px::new(vec![px::sl(&px::title(&long))]));
    assert_eq!(d.slides[0].title, "t".repeat(200));
    let exact = "u".repeat(200);
    let d = px::doc(&px::Px::new(vec![px::sl(&px::title(&exact))]));
    assert_eq!(d.slides[0].title, exact);
    // The name of an embedded object: 80 characters.
    for (n, want) in [(80, 80), (81, 80), (79, 79)] {
        let o = px::md1(&ole(&format!(
            r#"<p:oleObj progId="{}" r:id="rId9"><p:embed/></p:oleObj>"#,
            "p".repeat(n)
        )));
        assert_eq!(o.matches('p').count(), want, "{n}: {o}");
    }
    // The description of a picture: 300 characters.
    for (n, want) in [(300, 300), (301, 300), (299, 299)] {
        let m = px::md1(&pic_with(Some(&"z".repeat(n)), None));
        assert_eq!(m.matches('z').count(), want, "{n}");
    }
}

fn ole(inner: &str) -> String {
    format!(
        r#"<p:graphicFrame><p:nvGraphicFramePr><p:cNvPr id="90" name="Obj"/><p:cNvGraphicFramePr/><p:nvPr/></p:nvGraphicFramePr><p:xfrm><a:off x="0" y="0"/><a:ext cx="{I}" cy="{I}"/></p:xfrm><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/presentationml/2006/ole">{inner}</a:graphicData></a:graphic></p:graphicFrame>"#
    )
}

#[test]
fn an_object_deep_in_a_frame_is_found_down_to_a_depth() {
    // `p:oleObj` is looked for 12 elements deep (and a picture the same).
    let wrap = |n: usize| {
        let o = r#"<p:oleObj progId="Deep.Thing" r:id="rId9"><p:embed/></p:oleObj>"#;
        ole(&format!("{}{o}{}", "<x:w>".repeat(n), "</x:w>".repeat(n)))
    };
    let shown = |n: usize| px::md1(&wrap(n)).contains("Deep.Thing");
    let deepest = (0..20).take_while(|&n| shown(n)).last().unwrap();
    assert_eq!(deepest, 12, "the deepest wrapper count that still shows it");
}

// ---------------------------------------------------------------------------------------------
// inheritance: which placeholder a shape stands for
// ---------------------------------------------------------------------------------------------

fn layout_ph(attrs: &str, pos: (i64, i64, i64, i64)) -> String {
    px::ph(attrs, Some(pos), "")
}

#[test]
fn a_placeholder_without_an_idx_does_not_take_the_layouts_title_for_its_body() {
    // The layout has a title (idx 0) first and a body (idx 1): a slide body placeholder with no idx
    // (idx 0) must find the body by its kind, not the title by its number.
    let layout = [
        layout_ph(r#"type="title""#, (0, 0, 4 * I, I)),
        layout_ph(r#"type="body" idx="1""#, (0, 5 * I, 4 * I, I)),
    ]
    .concat();
    let s = [
        px::ph(r#"type="body""#, None, &px::pa("bodytext")),
        px::tb(0, 3 * I, I, I, &px::pa("middle")),
    ]
    .concat();
    let p = px::Px::new(vec![px::sl(&s)]).layout(0, &layout);
    // The body sits at 5 inches: after the box at 3.
    assert_eq!(
        order(&px::doc(&p).markdown, &["bodytext", "middle"]),
        ["middle", "bodytext"]
    );
}

#[test]
fn an_idx_finds_the_layout_placeholder_of_the_same_kind() {
    let layout = [
        layout_ph(r#"type="subTitle" idx="1""#, (0, 0, 4 * I, I)),
        layout_ph(r#"type="body" idx="1""#, (0, 5 * I, 4 * I, I)),
    ]
    .concat();
    let s = [
        px::ph(r#"type="body" idx="1""#, None, &px::pa("bodytext")),
        px::tb(0, 3 * I, I, I, &px::pa("middle")),
    ]
    .concat();
    let p = px::Px::new(vec![px::sl(&s)]).layout(0, &layout);
    assert_eq!(
        order(&px::doc(&p).markdown, &["bodytext", "middle"]),
        ["middle", "bodytext"]
    );
}

#[test]
fn a_placeholder_with_no_idx_in_a_layout_of_other_idx_finds_it_by_its_type() {
    let layout = layout_ph(r#"type="body" idx="2""#, (0, 5 * I, 4 * I, I));
    let s = [
        px::ph(r#"type="body""#, None, &px::pa("bodytext")),
        px::tb(0, 3 * I, I, I, &px::pa("middle")),
    ]
    .concat();
    let p = px::Px::new(vec![px::sl(&s)]).layout(0, &layout);
    // (Found by type: placed at 5 inches. Not found, it would be loose and last: the same order,
    // so the other way round is checked as well.)
    let s2 = [
        px::ph(r#"type="body""#, None, &px::pa("bodytext")),
        px::tb(0, 7 * I, I, I, &px::pa("low")),
    ]
    .concat();
    let p2 = px::Px::new(vec![px::sl(&s2)]).layout(0, &layout);
    assert_eq!(
        order(&px::doc(&p).markdown, &["bodytext", "middle"]),
        ["middle", "bodytext"]
    );
    assert_eq!(
        order(&px::doc(&p2).markdown, &["bodytext", "low"]),
        ["bodytext", "low"]
    );
}

#[test]
fn a_subtitle_kind_picture_and_a_title_kind_table_inherit_from_the_masters_body_and_title() {
    // A picture in a subtitle placeholder takes the master's *body* rectangle (a subtitle is a body
    // there); a table in a title placeholder takes the master's *title* one.
    let master = [
        px::ph(r#"type="title""#, Some((0, I, 4 * I, I)), &px::pa("Click")),
        px::ph(
            r#"type="body" idx="1""#,
            Some((0, 5 * I, 4 * I, I)),
            &px::pa("Click"),
        ),
    ]
    .concat();
    let sub_pic = |ph: &str| {
        format!(
            r#"<p:pic><p:nvPicPr><p:cNvPr id="40" name="P" descr="pictext"/><p:cNvPicPr/><p:nvPr><p:ph {ph}/></p:nvPr></p:nvPicPr><p:blipFill><a:blip r:embed="rIdNone"/></p:blipFill><p:spPr/></p:pic>"#
        )
    };
    let mid = px::tb(0, 3 * I, I, I, &px::pa("middle"));
    let mut p = px::Px::new(vec![px::sl(&(sub_pic(r#"type="subTitle""#) + &mid))]);
    p.master = master.clone();
    // The body is at 5 inches, below the middle box.
    assert_eq!(
        order(&px::doc(&p).markdown, &["pictext", "middle"]),
        ["middle", "pictext"]
    );
    // A picture in a title placeholder is at the master's title (1 inch): above the middle box.
    let mut p = px::Px::new(vec![px::sl(&(sub_pic(r#"type="title""#) + &mid))]);
    p.master = master;
    assert_eq!(
        order(&px::doc(&p).markdown, &["pictext", "middle"]),
        ["pictext", "middle"]
    );
}

#[test]
fn a_layout_placeholder_without_a_position_leaves_it_to_the_master_and_pictures_and_tables_inherit()
{
    // The layout's body has no `a:xfrm`: the master's body rectangle (5 inches) is used. The
    // layout also has a picture placeholder and a table (graphic frame) placeholder with positions.
    let layout = [
        px::ph(r#"type="body" idx="1""#, None, ""),
        format!(
            r#"<p:pic><p:nvPicPr><p:cNvPr id="3" name="LP"/><p:cNvPicPr/><p:nvPr><p:ph type="pic" idx="2"/></p:nvPr></p:nvPicPr><p:blipFill/><p:spPr>{}</p:spPr></p:pic>"#,
            px::xf(0, 6 * I, I, I)
        ),
        format!(
            r#"<p:graphicFrame><p:nvGraphicFramePr><p:cNvPr id="4" name="LT"/><p:cNvGraphicFramePr/><p:nvPr><p:ph type="tbl" idx="3"/></p:nvPr></p:nvGraphicFramePr><p:xfrm><a:off x="0" y="{}"/><a:ext cx="{I}" cy="{I}"/></p:xfrm></p:graphicFrame>"#,
            4 * I
        ),
    ]
    .concat();
    let pic = r#"<p:pic><p:nvPicPr><p:cNvPr id="40" name="P" descr="pictext"/><p:cNvPicPr/><p:nvPr><p:ph type="pic" idx="2"/></p:nvPr></p:nvPicPr><p:blipFill><a:blip r:embed="rIdNone"/></p:blipFill><p:spPr/></p:pic>"#.to_string();
    let tbl = format!(
        r#"<p:graphicFrame><p:nvGraphicFramePr><p:cNvPr id="50" name="T"/><p:cNvGraphicFramePr/><p:nvPr><p:ph type="tbl" idx="3"/></p:nvPr></p:nvGraphicFramePr><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/table"><a:tbl><a:tblPr/><a:tblGrid/>{}</a:tbl></a:graphicData></a:graphic></p:graphicFrame>"#,
        px::tr(&["tabletext"])
    );
    let body = px::ph(r#"type="body" idx="1""#, None, &px::pa("bodytext"));
    let mid = |y: i64, t: &str| px::tb(0, y, I, I, &px::pa(t));
    let s = [
        body,
        pic,
        tbl,
        mid(3 * I + I / 2, "m1"),
        mid(5 * I + I / 2, "m2"),
        mid(6 * I + I / 2, "m3"),
    ]
    .concat();
    let mut p = px::Px::new(vec![px::sl(&s)]).layout(0, &layout);
    p.master = px::ph(
        r#"type="body" idx="1""#,
        Some((0, 5 * I, I, I)),
        &px::pa("Click"),
    );
    // table 4in, m1 3.5in, body (master) 5in, m2 5.5in, pic 6in, m3 6.5in. The heights of the boxes
    // are 1in, so they touch each other and the order is by top.
    assert_eq!(
        order(
            &px::doc(&p).markdown,
            &["tabletext", "m1", "bodytext", "m2", "pictext", "m3"]
        ),
        ["m1", "tabletext", "bodytext", "m2", "pictext", "m3"]
    );
}

#[test]
fn a_layout_holds_two_hundred_placeholders_and_no_more() {
    // The layout has 201 placeholders, idx 1..=201, each at its own height; the slide's
    // placeholders of idx 200 and 201 stand for the last two. The 201st is not remembered.
    let layout: String = (1..=201)
        .map(|i| layout_ph(&format!(r#"type="body" idx="{i}""#), (0, 0, I, I)))
        .collect();
    let s = [
        px::ph(r#"type="body" idx="200""#, None, &px::pa("two00")),
        px::ph(r#"type="body" idx="201""#, None, &px::pa("two01")),
        px::tb(0, 3 * I, I, I, &px::pa("middle")),
    ]
    .concat();
    let p = px::Px::new(vec![px::sl(&s)]).layout(0, &layout);
    // idx 200 sits at the top; idx 201 is unplaced and last.
    assert_eq!(
        order(&px::doc(&p).markdown, &["two00", "two01", "middle"]),
        ["two00", "middle", "two01"]
    );
}

// ---------------------------------------------------------------------------------------------
// rotation and group transforms, told apart through the reading order
// ---------------------------------------------------------------------------------------------

fn rot_order(rot: i64) -> Vec<String> {
    // A 4 x 1 inch box at (0, 2in), turned by `rot`. A quarter turn makes it 1 x 4 inches about
    // its centre (x 1.5..2.5, y 0.5..4.5), which puts it left of the small box at x 3..4 (a
    // column) rather than below it (a stripe above, y 1.4..1.8): the order tells the two apart.
    let spp = format!(
        r#"<a:xfrm rot="{rot}"><a:off x="0" y="{}"/><a:ext cx="{}" cy="{I}"/></a:xfrm>"#,
        2 * I,
        4 * I
    );
    let s = [
        px::sp_raw(5, "", &spp, &px::pa("rotbox")),
        px::tb(3 * I, 7 * I / 5, I, 2 * I / 5, &px::pa("other")),
    ]
    .concat();
    order(&px::md1(&s), &["rotbox", "other"])
}

#[test]
fn a_shape_turned_a_quarter_is_placed_by_its_turned_box() {
    let turned = ["rotbox", "other"];
    let flat = ["other", "rotbox"];
    for rot in [
        5_400_000,
        2_700_000,  // 45 degrees: the edge is included
        8_099_999,  // just under 135
        13_500_000, // 225
        18_899_999, // just under 315
        16_200_000, // 270
        -5_400_000, // -90 is 270
        27_000_000, // 450 is 90
        -16_200_000,
    ] {
        assert_eq!(rot_order(rot), turned, "rot={rot}");
    }
    for rot in [
        0,
        1,
        2_699_999,
        8_100_000, // 135: the edge is not included
        10_800_000,
        13_499_999,
        18_900_000, // 315
        20_000_000,
        21_599_999,
        21_600_000, // a full turn
        -1,
        -2_699_999,
        -21_600_000,
        43_200_000,
    ] {
        assert_eq!(rot_order(rot), flat, "rot={rot}");
    }
}

#[test]
fn rotation_with_extreme_numbers_never_overflows() {
    let (mx, mn) = (i64::MAX, i64::MIN);
    let corners = [(mx, mx), (mn, mn), (mx, mn), (mn, mx), (0, mx), (mn, 0)];
    for rot in [0, 5_400_000, 16_200_000] {
        for &(x, y) in &corners {
            for &(w, h) in &corners {
                let spp = format!(
                    r#"<a:xfrm rot="{rot}"><a:off x="{x}" y="{y}"/><a:ext cx="{w}" cy="{h}"/></a:xfrm>"#
                );
                let s = [
                    px::sp_raw(5, "", &spp, &px::pa("huge")),
                    px::tb(0, 0, I, I, &px::pa("normal")),
                ]
                .concat();
                // (Tests run with overflow checks: an overflow is a panic, and a panic is `Corrupt`.)
                let d = px::load_px(&px::Px::new(vec![px::sl(&s)]), &DocOptions::default());
                let m =
                    d.unwrap_or_else(|e| panic!("rot={rot} off=({x},{y}) ext=({w},{h}): {e:?}"));
                assert!(m.markdown.contains("huge"), "{}", m.markdown);
            }
        }
    }
}

#[test]
fn a_group_with_extreme_numbers_never_overflows() {
    let (mx, mn) = (i64::MAX, i64::MIN);
    for off in [(mx, mx), (mn, mn), (1, 1)] {
        for ch in [(mn, mn), (mx, mx), (0, 0)] {
            for chext in [(1, 1), (mx, mx), (mn, mn), (0, 0)] {
                let g = px::group(
                    (off.0, off.1, mx, mx),
                    (ch.0, ch.1, chext.0, chext.1),
                    &px::tb(5000, 5000, 100, 100, &px::pa("kid")),
                );
                let s = g + &px::tb(0, 0, I, I, &px::pa("normal"));
                let d = px::load_px(&px::Px::new(vec![px::sl(&s)]), &DocOptions::default());
                let m =
                    d.unwrap_or_else(|e| panic!("off={off:?} chOff={ch:?} chExt={chext:?}: {e:?}"));
                assert!(m.markdown.contains("kid"), "{}", m.markdown);
            }
        }
    }
}

/// A group with only an `a:xfrm` `off` / `ext` (no child space), or none at all.
fn group_raw(xfrm: &str, inner: &str) -> String {
    format!(
        r#"<p:grpSp><p:nvGrpSpPr><p:cNvPr id="31" name="G"/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr>{xfrm}</p:grpSpPr>{inner}</p:grpSp>"#
    )
}

fn off_ext(x: i64, y: i64, w: i64, h: i64) -> String {
    format!(r#"<a:xfrm><a:off x="{x}" y="{y}"/><a:ext cx="{w}" cy="{h}"/></a:xfrm>"#)
}

#[test]
fn a_group_maps_its_members_from_the_child_space_to_the_slide() {
    // off (0, 2in, 8in x 4in), child space (1000, 1000, 100 x 100): a member at (1000, 1000, 100 x
    // 100) fills the group. It is read between a box above it and one below it; mapped wrongly it
    // would be far off the slide and read last.
    let k = px::group(
        (0, 2 * I, 8 * I, 4 * I),
        (1000, 1000, 100, 100),
        &px::tb(1000, 1000, 100, 100, &px::pa("kid")),
    );
    let s = [
        px::tb(2 * I, 0, I, I, &px::pa("first")),
        k,
        px::tb(0, 7 * I, I, I, &px::pa("last")),
    ]
    .concat();
    assert_eq!(
        order(&px::md1(&s), &["first", "kid", "last"]),
        ["first", "kid", "last"]
    );
}

#[test]
fn a_member_is_scaled_in_width_by_the_width_scale() {
    // off (2in, 2in, 8in x 1in), child space (0,0,100,100): a member 50 wide is 4in wide: it
    // overlaps the box above it in x, so they are a stripe (above first); were it 50 EMU wide it
    // would be left of it in a column.
    let k = px::group(
        (2 * I, 2 * I, 8 * I, I),
        (0, 0, 100, 100),
        &px::tb(0, 0, 50, 100, &px::pa("kid")),
    );
    let s = [k, px::tb(4 * I, 0, I, I, &px::pa("cee"))].concat();
    assert_eq!(order(&px::md1(&s), &["kid", "cee"]), ["cee", "kid"]);
}

#[test]
fn a_member_is_scaled_in_height_by_the_height_scale() {
    // off (2in, 0, 1in x 4in): a member 100 high is 4in high, overlapping the box at the left in y
    // (a column); at 100 EMU it would be a stripe above it.
    let k = px::group(
        (2 * I, 0, I, 4 * I),
        (0, 0, 100, 100),
        &px::tb(0, 0, 100, 100, &px::pa("kid")),
    );
    let s = [k, px::tb(0, 2 * I, I, I, &px::pa("cee"))].concat();
    assert_eq!(order(&px::md1(&s), &["kid", "cee"]), ["cee", "kid"]);
}

#[test]
fn x_is_scaled_by_the_x_scale_and_y_by_the_y_scale() {
    // The scales differ (8in / 100 in x, 2in / 100 in y). A member at (50, 50): x = 4in, y = 1in.
    let g = || {
        px::group(
            (0, 0, 8 * I, 2 * I),
            (0, 0, 100, 100),
            &px::tb(50, 50, 5, 5, &px::pa("kid")),
        )
    };
    // x: left of it (2.5..3.5in) is the box "cee"; with the y scale (1in) it would be right of
    // the box's left edge instead.
    let s = [g(), px::tb(5 * I / 2, I, I, I, &px::pa("cee"))].concat();
    assert_eq!(order(&px::md1(&s), &["kid", "cee"]), ["cee", "kid"]);
    // y: the member is at 1in; the box "dee" at 2.5..3.5in is below it (with the x scale, 4in, the
    // member would be below the box).
    let s = [g(), px::tb(4 * I, 5 * I / 2, I, I, &px::pa("dee"))].concat();
    assert_eq!(order(&px::md1(&s), &["kid", "dee"]), ["kid", "dee"]);
}

#[test]
fn a_group_inside_a_group_without_a_transform_keeps_the_outer_one() {
    let inner = |xfrm: &str| group_raw(xfrm, &px::tb(1000, 1000, 100, 100, &px::pa("kid")));
    for xfrm in [
        String::new(),
        // `off` without `ext` is no transform either.
        r#"<a:xfrm><a:off x="5" y="5"/></a:xfrm>"#.to_string(),
        r#"<a:xfrm><a:ext cx="5" cy="5"/></a:xfrm>"#.to_string(),
    ] {
        let k = px::group(
            (0, 2 * I, 8 * I, 4 * I),
            (1000, 1000, 100, 100),
            &inner(&xfrm),
        );
        let s = [
            px::tb(2 * I, 0, I, I, &px::pa("first")),
            k,
            px::tb(0, 7 * I, I, I, &px::pa("last")),
        ]
        .concat();
        assert_eq!(
            order(&px::md1(&s), &["first", "kid", "last"]),
            ["first", "kid", "last"],
            "{xfrm:?}"
        );
    }
}

#[test]
fn a_group_with_no_child_space_leaves_its_members_where_they_are() {
    // `off` / `ext` only: the members use the group's own coordinates.
    let g = group_raw(
        &off_ext(4 * I, 0, 2 * I, 2 * I),
        &px::tb(4 * I, 0, I, I, &px::pa("kid")),
    );
    let s = [g, px::tb(13 * I / 2, 0, I, I, &px::pa("cee"))].concat();
    assert_eq!(order(&px::md1(&s), &["kid", "cee"]), ["kid", "cee"]);
}

#[test]
fn a_child_space_that_is_not_positive_does_not_scale_the_members() {
    // chExt 0 and chExt negative: scale 1 (never 0, never infinite).
    for ext in [(0, 0), (-100, -100)] {
        let g = px::group(
            (0, 0, I, I),
            (0, 0, ext.0, ext.1),
            &px::tb(3 * I, 0, I, I, &px::pa("kid")),
        );
        let s = [g, px::tb(3 * I / 2, 0, I, I, &px::pa("cee"))].concat();
        assert_eq!(
            order(&px::md1(&s), &["kid", "cee"]),
            ["cee", "kid"],
            "chExt={ext:?}"
        );
    }
}

#[test]
fn a_big_scale_is_kept_and_not_cut_to_one() {
    // 1,000,000 times (the most): a member at (1,0) of 1 EMU is at 1,000,000 EMU = 1.09in.
    let g = px::group(
        (0, 0, 1_000_000, 1_000_000),
        (0, 0, 1, 1),
        &px::tb(1, 0, 1, 1, &px::pa("kid")),
    );
    // A box at 0.5..1.0in is left of the member (right of 0): with the scale cut to 1 the member
    // would be at x=1 EMU, left of the box.
    let s = [g, px::tb(I / 2, 0, I / 2, I, &px::pa("cee"))].concat();
    assert_eq!(order(&px::md1(&s), &["kid", "cee"]), ["cee", "kid"]);
}

// ---------------------------------------------------------------------------------------------
// depth and size budgets, at their edges
// ---------------------------------------------------------------------------------------------

fn nest(depth: usize, leaf: &str) -> String {
    let mut s = leaf.to_string();
    for _ in 0..depth {
        s = px::group((0, 0, 100, 100), (0, 0, 100, 100), &s);
    }
    s
}

fn nest_ac(depth: usize, leaf: &str) -> String {
    let mut s = leaf.to_string();
    for _ in 0..depth {
        s = format!(
            r#"<mc:AlternateContent><mc:Choice Requires="a14">{s}</mc:Choice></mc:AlternateContent>"#
        );
    }
    s
}

#[test]
fn groups_are_followed_twenty_deep_and_a_shape_of_any_kind_below_that_is_reported() {
    let text = px::tb(0, 0, 10, 10, &px::pa("deep"));
    let pic = pic_with(Some("deeppic"), None);
    let table = px::table((0, 0, 10, 10), &px::tr(&["deeptable"]));
    for (leaf, word) in [(&text, "deep"), (&pic, "deeppic"), (&table, "deeptable")] {
        let at = px::doc(&px::Px::new(vec![px::sl(&nest(20, leaf))]));
        assert!(at.markdown.contains(word), "{word}: {}", at.markdown);
        assert!(!at.truncated, "{word}");
        let past = px::doc(&px::Px::new(vec![px::sl(&nest(21, leaf))]));
        assert!(!past.markdown.contains(word), "{word}: {}", past.markdown);
        assert!(past.truncated, "{word}");
    }
    // A group below the depth followed counts as well (it holds shapes).
    let past = px::doc(&px::Px::new(vec![px::sl(&nest(22, &text))]));
    assert!(past.truncated);
    // A connector is not in the text view, but the picture view draws it: one lost below the depth
    // followed is reported as well.
    let past = px::doc(&px::Px::new(vec![px::sl(&nest(21, &px::connector()))]));
    assert!(past.truncated);
}

#[test]
fn alternate_content_nests_twenty_deep_like_groups() {
    let text = px::tb(0, 0, 10, 10, &px::pa("deep"));
    let at = px::doc(&px::Px::new(vec![px::sl(&nest_ac(20, &text))]));
    assert!(at.markdown.contains("deep"), "{}", at.markdown);
    assert!(!at.truncated);
    let past = px::doc(&px::Px::new(vec![px::sl(&nest_ac(21, &text))]));
    assert!(!past.markdown.contains("deep"), "{}", past.markdown);
    assert!(past.truncated);
}

#[test]
fn inline_wrappers_are_followed_eight_deep() {
    let run = |n: usize| {
        let inner = format!(
            "{}{}{}",
            "<a14:m>".repeat(n),
            "<a:r><a:t>deepword</a:t></a:r>",
            "</a14:m>".repeat(n)
        );
        px::body1(&px::tb(0, 0, I, I, &px::paras(&inner)))
    };
    assert_eq!(run(8), "deepword");
    assert_eq!(run(9), "");
}

#[test]
fn the_shapes_of_a_slide_are_counted_with_the_members_of_its_groups() {
    let kid = |i: usize| px::tb(0, i as i64 * 10, 5, 5, &px::pa(&format!("kid{i}")));
    let group_of = |n: usize| {
        let inner: String = (1..=n).map(kid).collect();
        px::group((0, 0, 1000, 1000), (0, 0, 1000, 1000), &inner)
    };
    let opts = opts_with(|o| o.max_slide_shapes = 3);
    // The group is one shape and its two members two more: exactly the budget.
    let at = px::load_px(&px::Px::new(vec![px::sl(&group_of(2))]), &opts).unwrap();
    assert!(at.markdown.contains("kid1") && at.markdown.contains("kid2"));
    assert!(!at.truncated);
    // A third member is over it.
    let over = px::load_px(&px::Px::new(vec![px::sl(&group_of(3))]), &opts).unwrap();
    assert!(over.markdown.contains("kid2"), "{}", over.markdown);
    assert!(!over.markdown.contains("kid3"), "{}", over.markdown);
    assert!(over.truncated);
}

#[test]
fn a_slide_keeps_as_many_shapes_as_the_budget_and_says_so_for_the_next() {
    let opts = opts_with(|o| o.max_slide_shapes = 3);
    let boxes = |n: usize| -> String {
        (1..=n)
            .map(|i| px::tb(0, i as i64 * I, I, I / 2, &px::pa(&format!("box{i}"))))
            .collect()
    };
    let at = px::load_px(&px::Px::new(vec![px::sl(&boxes(3))]), &opts).unwrap();
    assert!(at.markdown.contains("box3") && !at.truncated);
    let over = px::load_px(&px::Px::new(vec![px::sl(&boxes(4))]), &opts).unwrap();
    assert!(over.markdown.contains("box3"), "{}", over.markdown);
    assert!(!over.markdown.contains("box4"), "{}", over.markdown);
    assert!(over.truncated);
    // A slide with none is not "cut" at a budget of 0.
    let none = opts_with(|o| o.max_slide_shapes = 0);
    let d = px::load_px(&px::Px::new(vec![px::sl(&boxes(1))]), &none).unwrap();
    assert!(!d.markdown.contains("box1") && d.truncated);
}

#[test]
fn a_deck_is_cut_at_its_slide_budget_exactly() {
    let opts = opts_with(|o| o.max_slides = 3);
    let deck = |n: usize| {
        px::Px::new(
            (1..=n)
                .map(|i| px::sl(&px::title(&format!("T{i}"))))
                .collect(),
        )
    };
    let at = px::load_px(&deck(3), &opts).unwrap();
    assert_eq!(at.slides.len(), 3);
    assert!(!at.truncated);
    let over = px::load_px(&deck(4), &opts).unwrap();
    assert_eq!(over.slides.len(), 3);
    assert!(over.truncated);
    assert!(!over.markdown.contains("T4"));
}

/// The slide, padded with a comment, so that it is the biggest part of the package.
fn padded() -> Vec<(String, Vec<u8>)> {
    let p = px::Px::new(vec![px::sl(&px::tb(0, 0, I, I, &px::pa("hello")))]);
    let mut e = p.entries();
    edit_part(&mut e, "ppt/slides/slide1.xml", |s| {
        s.replacen(
            "<p:cSld>",
            &format!("<!--{}--><p:cSld>", "c".repeat(3000)),
            1,
        )
    });
    e
}

#[test]
fn a_part_of_exactly_the_size_cap_is_read_and_one_byte_more_is_not() {
    let e = padded();
    let len = part_len(&e, "ppt/slides/slide1.xml");
    for name in ["ppt/presentation.xml", "ppt/slideLayouts/slideLayout1.xml"] {
        assert!(part_len(&e, name) < len, "{name} must be smaller");
    }
    let at = load_entries(&e, &opts_with(|o| o.max_slide_part_bytes = len)).unwrap();
    assert!(
        at.markdown.contains("hello") && !at.truncated,
        "{}",
        at.markdown
    );
    let over = load_entries(&e, &opts_with(|o| o.max_slide_part_bytes = len - 1)).unwrap();
    assert!(!over.markdown.contains("hello"), "{}", over.markdown);
    assert!(over.truncated);
    // The slide keeps its place in the order, as a bare heading.
    assert_eq!(over.markdown, "## Slide 1");
}

#[test]
fn the_read_budget_of_a_deck_is_the_sum_of_the_parts_it_reads() {
    let p = px::Px::new(vec![px::sl(&px::tb(0, 0, I, I, &px::pa("hello")))]);
    let e = p.entries();
    // The slide, its relationships, the layout, its relationships and the master (the
    // presentation part and the package's own relationships are read apart).
    let total: u64 = [
        "ppt/slides/slide1.xml",
        "ppt/slides/_rels/slide1.xml.rels",
        "ppt/slideLayouts/slideLayout1.xml",
        "ppt/slideLayouts/_rels/slideLayout1.xml.rels",
        "ppt/slideMasters/slideMaster1.xml",
    ]
    .iter()
    .map(|n| part_len(&e, n))
    .sum();
    let at = load_entries(&e, &opts_with(|o| o.max_pptx_read_total = total)).unwrap();
    assert!(!at.truncated, "{}", at.markdown);
    let over = load_entries(&e, &opts_with(|o| o.max_pptx_read_total = total - 1)).unwrap();
    assert!(over.truncated);
    // Nothing at all to read with: the slide is a bare heading.
    let none = load_entries(&e, &opts_with(|o| o.max_pptx_read_total = 0)).unwrap();
    assert!(none.truncated);
    assert_eq!(none.markdown, "## Slide 1");
}

fn notes_deck(n: usize) -> px::Px {
    let bodies: String = (1..=n)
        .map(|i| {
            px::sp_raw(
                i as u32,
                r#"<p:ph type="body" idx="1"/>"#,
                "",
                &px::pa(&format!("note{i}")),
            )
        })
        .collect();
    px::Px::new(vec![px::sl(&px::title("T")).notes(&bodies)])
}

#[test]
fn eight_body_placeholders_of_a_notes_page_are_read_and_the_ninth_is_reported() {
    let at = px::doc(&notes_deck(8));
    for i in 1..=8 {
        assert!(at.markdown.contains(&format!("note{i}")), "{}", at.markdown);
    }
    assert!(!at.truncated);
    let over = px::doc(&notes_deck(9));
    assert!(over.markdown.contains("note8"), "{}", over.markdown);
    assert!(!over.markdown.contains("note9"), "{}", over.markdown);
    assert!(over.truncated);
}

#[test]
fn a_notes_part_over_the_node_budget_is_reported_and_so_is_a_broken_one() {
    // The slide is small; the notes page has hundreds of paragraphs.
    let paras: String = (0..300).map(|i| px::pa(&format!("n{i}"))).collect();
    let p = px::Px::new(vec![px::sl(&px::title("T")).notes(&px::sp_raw(
        3,
        r#"<p:ph type="body" idx="1"/>"#,
        "",
        &paras,
    ))]);
    let opts = opts_with(|o| o.max_block_nodes = 600);
    let d = px::load_px(&p, &opts).unwrap();
    assert!(d.truncated, "{}", d.markdown);
    // The same notes are fine under the default budget.
    assert!(!px::doc(&p).truncated);
    // A notes part that is not well-formed XML (an end tag that does not match).
    let p = px::Px::new(vec![px::sl(&px::title("T")).notes(&px::sp_raw(
        3,
        r#"<p:ph type="body" idx="1"/>"#,
        "",
        &px::pa("note"),
    ))]);
    let d = load_edited(&p, &DocOptions::default(), |e| {
        edit_part(e, "ppt/notesSlides/notesSlide1.xml", |s| {
            s.replace("</p:notes>", "</p:oops>")
        })
    })
    .unwrap();
    assert!(d.truncated);
    px::check_headings(&d);
}

#[test]
fn only_a_shape_that_is_a_body_placeholder_is_a_note() {
    // A picture that carries an `nvSpPr` child (a forged one) is not a text shape.
    let forged = r#"<p:pic><p:nvSpPr><p:cNvPr id="9" name="x"/><p:cNvSpPr/><p:nvPr><p:ph type="body" idx="1"/></p:nvPr></p:nvSpPr><p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:t>forged note</a:t></a:r></a:p></p:txBody></p:pic>"#;
    let p = px::Px::new(vec![px::sl(&px::title("T")).notes(forged)]);
    let d = px::doc(&p);
    assert!(!d.markdown.contains("forged note"), "{}", d.markdown);
}

#[test]
fn notes_keep_their_line_breaks_inside_the_quote_and_skip_blank_paragraphs() {
    let tx = [
        px::pa("one"),
        px::pa("   "),
        px::paras(&format!(
            "{}<a:br/>{}",
            px::ru("", "two"),
            px::ru("", "three")
        )),
    ]
    .concat();
    let p = px::Px::new(vec![px::sl(&px::title("T")).notes(&px::sp_raw(
        3,
        r#"<p:ph type="body" idx="1"/>"#,
        "",
        &tx,
    ))]);
    let d = px::doc(&p);
    assert_eq!(
        d.markdown,
        "## Slide 1: T\n\n> **Notes**  \n> one  \n> two  \n> three"
    );
}

// ---------------------------------------------------------------------------------------------
// the cancel hook and a panic
// ---------------------------------------------------------------------------------------------

fn counting_cancel(after: usize) -> (Cancel, Arc<AtomicUsize>) {
    let n = Arc::new(AtomicUsize::new(0));
    let n2 = n.clone();
    (
        Cancel::new(move || n2.fetch_add(1, Ordering::Relaxed) >= after),
        n,
    )
}

#[test]
fn cancelling_stops_the_shapes_of_a_slide_one_by_one() {
    let boxes = |n: usize| -> String {
        (0..n)
            .map(|i| {
                px::tb(
                    0,
                    i as i64 * (I / 4),
                    I,
                    I / 8,
                    &px::pa(&format!("shape{i:02}")),
                )
            })
            .collect()
    };
    let dir = tmp("pptxcnl");
    let run = |n: usize, after: usize| {
        let p = write(
            &dir,
            "t.pptx",
            &px::Px::new(vec![px::sl(&boxes(n))]).bytes(),
        );
        let (c, count) = counting_cancel(after);
        let d = load_presentation_cancellable(&p, &DocOptions::default(), Some(&c)).unwrap();
        (d, count.load(Ordering::Relaxed))
    };
    // How many times the hook is asked before the first shape, and per shape, when it never says so.
    let (_, none) = run(0, usize::MAX);
    let (full, sixty) = run(60, usize::MAX);
    assert!(!full.truncated);
    assert!(
        sixty > none,
        "the hook is asked per shape ({none} vs {sixty})"
    );
    // Cancelled twenty questions after the slide was read: the first shapes are written, not all.
    let (d, _) = run(60, none + 20);
    let written = (0..60)
        .filter(|i| d.markdown.contains(&format!("shape{i:02}")))
        .count();
    assert!(d.truncated);
    assert!(written > 0 && written < 60, "{written} shapes written");
}

#[test]
fn a_panic_inside_the_reader_is_a_corrupt_file_not_a_crash() {
    let boom = Cancel::new(|| panic!("boom"));
    let dir = tmp("pptxpanic");
    let p = write(
        &dir,
        "t.pptx",
        &px::Px::new(vec![px::sl(&px::title("T"))]).bytes(),
    );
    match load_presentation_cancellable(&p, &DocOptions::default(), Some(&boom)) {
        Err(OfficeError::Corrupt(m)) => assert!(m.contains("panic"), "{m}"),
        other => panic!("{:?}", other.map(|d| d.markdown)),
    }
    let o = od::Op::new(&od::page(&od::title("T")));
    let p = write(&dir, "t.odp", &o.bytes());
    match load_presentation_cancellable(&p, &DocOptions::default(), Some(&boom)) {
        Err(OfficeError::Corrupt(m)) => assert!(m.contains("panic"), "{m}"),
        other => panic!("{:?}", other.map(|d| d.markdown)),
    }
}

// ---------------------------------------------------------------------------------------------
// relationships that are not in the package are not followed
// ---------------------------------------------------------------------------------------------

/// Makes the relationship of type `kind` in the relationships part `rels` an external one whose
/// target is *the name of a real part of the package* (an external target is never resolved, so a
/// reader that follows it anyway would find that part).
fn external(e: &mut [(String, Vec<u8>)], rels: &str, kind: &str) {
    let base = rels.rsplit_once("/_rels/").unwrap().0.to_string();
    edit_part(e, rels, |s| {
        let at = s
            .find(&format!("/{kind}\""))
            .unwrap_or_else(|| panic!("{kind} in {s}"));
        let start = s[..at].rfind("<Relationship").unwrap();
        let end = s[at..].find("/>").unwrap() + at;
        let el = &s[start..end];
        let t0 = el.find("Target=\"").unwrap() + "Target=\"".len();
        let t1 = t0 + el[t0..].find('"').unwrap();
        let mut path: Vec<&str> = base.split('/').collect();
        for seg in el[t0..t1].split('/') {
            match seg {
                ".." => {
                    path.pop();
                }
                "." | "" => {}
                other => path.push(other),
            }
        }
        let abs = path.join("/");
        format!(
            "{}{}{}{}",
            &s[..start + t0],
            abs,
            &s[start + t1..end],
            format_args!(" TargetMode=\"External\"{}", &s[end..])
        )
    });
}

#[test]
fn an_external_slide_relationship_is_a_slide_that_cannot_be_read() {
    let p = px::Px::new(vec![px::sl(&px::tb(0, 0, I, I, &px::pa("hello")))]);
    let d = load_edited(&p, &DocOptions::default(), |e| {
        external(e, "ppt/_rels/presentation.xml.rels", "slide")
    })
    .unwrap();
    assert_eq!(d.markdown, "## Slide 1");
    assert!(d.truncated);
    assert_eq!(d.slides.len(), 1);
}

#[test]
fn an_external_officedocument_relationship_is_not_the_main_part() {
    // The package's own relationships point at a decoy (a presentation with no slides) with
    // `TargetMode="External"`: the real one is found by its place.
    let p = px::Px::new(vec![px::sl(&px::title("Real"))]);
    let d = load_edited(&p, &DocOptions::default(), |e| {
        let decoy = e
            .iter()
            .find(|(n, _)| n == "ppt/presentation.xml")
            .unwrap()
            .1
            .clone();
        let decoy = String::from_utf8(decoy)
            .unwrap()
            .replace(r#"<p:sldId id="256" r:id="rIdS0"/>"#, "");
        e.push(("ppt/decoy.xml".into(), decoy.into_bytes()));
        edit_part(e, "_rels/.rels", |s| {
            s.replace(
                r#"Target="ppt/presentation.xml""#,
                r#"Target="ppt/decoy.xml" TargetMode="External""#,
            )
        });
    })
    .unwrap();
    assert_eq!(d.markdown, "## Slide 1: Real");
}

fn body_ph_slide() -> px::Px {
    // The body placeholder takes its bullet from the master's body style.
    px::Px::new(vec![px::sl(&px::body((0, 0, I, I), &px::pa("x")))])
}

#[test]
fn the_master_layout_and_notes_of_a_slide_must_be_inside_the_package() {
    // Reference: with everything in place the body has the master's bullet.
    assert_eq!(px::doc(&body_ph_slide()).markdown, "## Slide 1\n\n- x");
    // The layout's master is external: no master, no bullet.
    let d = load_edited(&body_ph_slide(), &DocOptions::default(), |e| {
        external(
            e,
            "ppt/slideLayouts/_rels/slideLayout1.xml.rels",
            "slideMaster",
        )
    })
    .unwrap();
    assert_eq!(d.markdown, "## Slide 1\n\nx");
    // The slide's layout is external: no layout, no master either.
    let d = load_edited(&body_ph_slide(), &DocOptions::default(), |e| {
        external(e, "ppt/slides/_rels/slide1.xml.rels", "slideLayout")
    })
    .unwrap();
    assert_eq!(d.markdown, "## Slide 1\n\nx");
    // The slide's notes are external: no notes.
    let p = px::Px::new(vec![px::sl(&px::title("T")).notes(&px::sp_raw(
        3,
        r#"<p:ph type="body" idx="1"/>"#,
        "",
        &px::pa("secret note"),
    ))]);
    assert!(px::doc(&p).markdown.contains("secret note"));
    let d = load_edited(&p, &DocOptions::default(), |e| {
        external(e, "ppt/slides/_rels/slide1.xml.rels", "notesSlide")
    })
    .unwrap();
    assert_eq!(d.markdown, "## Slide 1: T");
}

#[test]
fn smartart_parts_must_be_inside_the_package() {
    let s = px::smart_frame("rIdDm", (0, 0, 4 * I, 3 * I));
    let mk = || {
        px::Px::new(vec![px::sl(&s)
            .rel("rIdDm", "diagramData", "../diagrams/data1.xml", false)
            .rel("rIdDr", "diagramDrawing", "../diagrams/drawing1.xml", false)])
        .part(
            "ppt/diagrams/data1.xml",
            &px::dm(&["from data"], Some("rIdDr")),
        )
        .part(
            "ppt/diagrams/drawing1.xml",
            &px::drawing(&[(0, 0, "from drawing")]),
        )
    };
    assert_eq!(px::doc(&mk()).markdown, "## Slide 1\n\n- from drawing");
    // The drawing is external: the data's text.
    let d = load_edited(&mk(), &DocOptions::default(), |e| {
        external(e, "ppt/slides/_rels/slide1.xml.rels", "diagramDrawing")
    })
    .unwrap();
    assert_eq!(d.markdown, "## Slide 1\n\n- from data");
    // The data is external: nothing to read.
    let d = load_edited(&mk(), &DocOptions::default(), |e| {
        external(e, "ppt/slides/_rels/slide1.xml.rels", "diagramData")
    })
    .unwrap();
    assert_eq!(d.markdown, "## Slide 1\n\n\\[SmartArt]");
}

// ---------------------------------------------------------------------------------------------
// SmartArt: paragraphs, and the 500 limits
// ---------------------------------------------------------------------------------------------

fn smart_deck(data: &[u8], drawing: Option<&[u8]>) -> px::Px {
    let s = px::smart_frame("rIdDm", (0, 0, 4 * I, 3 * I));
    let mut sl = px::sl(&s).rel("rIdDm", "diagramData", "../diagrams/data1.xml", false);
    if drawing.is_some() {
        sl = sl.rel("rIdDr", "diagramDrawing", "../diagrams/drawing1.xml", false);
    }
    let mut p = px::Px::new(vec![sl]).part("ppt/diagrams/data1.xml", data);
    if let Some(d) = drawing {
        p = p.part("ppt/diagrams/drawing1.xml", d);
    }
    p
}

#[test]
fn a_smartart_node_of_two_paragraphs_is_one_item_with_a_line_break() {
    let data = format!(
        r#"<?xml version="1.0"?><dgm:dataModel {}><dgm:ptLst><dgm:pt modelId="1"><dgm:t><a:bodyPr/><a:p><a:r><a:t>first line</a:t></a:r></a:p><a:p><a:r><a:t>second line</a:t></a:r></a:p></dgm:t></dgm:pt></dgm:ptLst></dgm:dataModel>"#,
        px::PNS
    );
    assert_eq!(
        px::doc(&smart_deck(data.as_bytes(), None)).markdown,
        "## Slide 1\n\n- first line  \n  second line"
    );
}

#[test]
fn smartart_text_is_cut_at_500_characters() {
    for (n, want) in [(500, 500), (501, 500), (499, 499)] {
        let t = "x".repeat(n);
        let d = px::doc(&smart_deck(&px::dm(&[t.as_str()], None), None));
        assert_eq!(d.markdown.matches('x').count(), want, "{n}");
    }
}

#[test]
fn a_smartart_drawing_is_read_for_500_shapes_and_a_data_model_for_500_points() {
    // 501 shapes in the drawing: 500 are listed.
    let shapes: Vec<(i64, i64, String)> = (0..501)
        .map(|i| (i * 200_000, 0, format!("s{i}")))
        .collect();
    let refs: Vec<(i64, i64, &str)> = shapes
        .iter()
        .map(|(x, y, t)| (*x, *y, t.as_str()))
        .collect();
    let no_text: [&str; 0] = [];
    let d = px::doc(&smart_deck(
        &px::dm(&no_text, Some("rIdDr")),
        Some(&px::drawing(&refs)),
    ));
    assert_eq!(
        d.markdown.matches("\n- ").count(),
        500,
        "{}",
        d.markdown.len()
    );
    assert!(d.markdown.contains("s499") && !d.markdown.contains("s500"));
    // The data model: the document point and the 499 first nodes use up the 500 points.
    for (n, want) in [(498, 498), (499, 499), (500, 499), (600, 499)] {
        let texts: Vec<String> = (0..n).map(|i| format!("n{i}")).collect();
        let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
        let d = px::doc(&smart_deck(&px::dm(&refs, None), None));
        assert_eq!(d.markdown.matches("\n- ").count(), want, "{n}");
    }
}

// ---------------------------------------------------------------------------------------------
// slide numbers, slide size
// ---------------------------------------------------------------------------------------------

fn number_slide() -> String {
    px::tb(
        0,
        0,
        2 * I,
        I,
        &px::paras(&format!(
            r#"{}<a:fld id="{{B6F15528-21DE-4FAA-801E-634DDDAF4B2B}}" type="slidenum"><a:rPr lang="en-US"/><a:t>‹#›</a:t></a:fld>"#,
            px::ru("", "page ")
        )),
    )
}

fn numbers(first: Option<&str>, slides: usize) -> Vec<String> {
    let t = number_slide();
    let p = px::Px::new((0..slides).map(|_| px::sl(&t)).collect());
    let d = load_edited(&p, &DocOptions::default(), |e| {
        if let Some(f) = first {
            edit_part(e, "ppt/presentation.xml", |s| {
                s.replace(
                    "<p:presentation ",
                    &format!(r#"<p:presentation firstSlideNum="{f}" "#),
                )
            })
        }
    })
    .unwrap();
    d.markdown
        .lines()
        .filter(|l| l.starts_with("page "))
        .map(str::to_string)
        .collect()
}

#[test]
fn slide_numbers_start_at_one_or_where_the_deck_says_within_range() {
    assert_eq!(numbers(None, 3), ["page 1", "page 2", "page 3"]);
    assert_eq!(numbers(Some("5"), 2), ["page 5", "page 6"]);
    assert_eq!(numbers(Some("0"), 2), ["page 0", "page 1"]);
    assert_eq!(numbers(Some("-5"), 2), ["page 0", "page 1"]);
    assert_eq!(numbers(Some("abc"), 2), ["page 1", "page 2"]);
    assert_eq!(
        numbers(Some("1000000"), 2),
        ["page 1000000", "page 1000001"]
    );
    assert_eq!(
        numbers(Some("2000000"), 2),
        ["page 1000000", "page 1000001"]
    );
}

#[test]
fn a_slide_listed_twice_in_a_row_is_written_twice_with_its_pictures() {
    // The second reference is written from the slide kept for it: its relationships (the picture)
    // and its own number must still be there.
    let pic = px::pic("rId5", "photo", Some((0, I, 2 * I, 2 * I)));
    let p = px::Px::new(vec![px::sl(&(number_slide() + &pic)).rel(
        "rId5",
        "image",
        "../media/f.png",
        false,
    )])
    .media("f.png", super::tests_docx::tiny_png(1));
    let d = load_edited(&p, &DocOptions::default(), |e| {
        edit_part(e, "ppt/presentation.xml", |s| {
            s.replace(
                r#"<p:sldId id="256" r:id="rIdS0"/>"#,
                &r#"<p:sldId id="256" r:id="rIdS0"/>"#.repeat(3),
            )
        })
    })
    .unwrap();
    assert_eq!(d.slides.len(), 3);
    assert_eq!(
        d.markdown.matches("![photo](office-img://").count(),
        3,
        "{}",
        d.markdown
    );
    for n in 1..=3 {
        assert!(d.markdown.contains(&format!("page {n}")), "{}", d.markdown);
    }
}

fn sized_deck(shapes: &str) -> px::Px {
    px::Px::new(vec![px::sl(shapes)])
}

#[test]
fn the_slide_size_is_width_by_height_not_the_other_way_round() {
    // 9144000 x 6858000 (10 x 7.5 inches). "pee" at x 8..9in is on the slide; "kew" overlaps it in x
    // and is below it: read as a stripe, pee first. With the sides swapped (7.5 x 10) pee would be
    // off the slide and read last.
    let s = [
        px::tb(8 * I, 0, I, I, &px::pa("pee")),
        px::tb(15 * I / 2, 3 * I, 3 * I / 2, I, &px::pa("kew")),
    ]
    .concat();
    assert_eq!(
        order(&px::doc(&sized_deck(&s)).markdown, &["pee", "kew"]),
        ["pee", "kew"]
    );
    // "vee" at y=8in is below the 7.5in slide: read after "why" (on it), though the position order
    // would put it first (columns); with the sides swapped (10in high) it would be on the slide.
    let s = [
        px::tb(0, 8 * I, I, I, &px::pa("vee")),
        px::tb(5 * I, 7 * I, I, I, &px::pa("why")),
    ]
    .concat();
    assert_eq!(
        order(&px::doc(&sized_deck(&s)).markdown, &["vee", "why"]),
        ["why", "vee"]
    );
}

// ---------------------------------------------------------------------------------------------
// mc:AlternateContent: every namespace the readers say they understand
// ---------------------------------------------------------------------------------------------

const PPTX_READS_NOW: [&str; 8] = ["a", "p", "r", "c", "dgm", "dsp", "m", "a14"];

const DOCX_READS_NOW: [&str; 21] = [
    "w", "wp", "a", "pic", "r", "m", "v", "o", "wps", "wpg", "wpc", "wp14", "w14", "w15", "w16",
    "w16se", "w16cid", "w16du", "w16sdtdh", "w16sdtfl", "a14",
];

fn ac_slide(requires: &str) -> String {
    format!(
        r#"<mc:AlternateContent><mc:Choice Requires="{requires}">{}</mc:Choice><mc:Fallback>{}</mc:Fallback></mc:AlternateContent>"#,
        px::tb(0, 0, I, I, &px::pa("choice")),
        px::tb(0, 0, I, I, &px::pa("fallback"))
    )
}

fn ac_inline(requires: &str) -> String {
    px::paras(&format!(
        r#"{}<mc:AlternateContent><mc:Choice Requires="{requires}"><a:r><a:t>choice</a:t></a:r></mc:Choice><mc:Fallback><a:r><a:t>fallback</a:t></a:r></mc:Fallback></mc:AlternateContent>"#,
        px::ru("", "x ")
    ))
}

#[test]
fn a_presentation_reads_each_namespace_it_names_in_a_choice() {
    for p in PPTX_READS_NOW {
        assert_eq!(px::body1(&ac_slide(p)), "choice", "shape level, {p}");
        assert_eq!(
            px::body1(&px::tb(0, 0, I, I, &ac_inline(p))),
            "x choice",
            "inline, {p}"
        );
    }
    // Two of them together; the same one twice.
    assert_eq!(px::body1(&ac_slide("a14 m")), "choice");
    assert_eq!(px::body1(&ac_slide("p a")), "choice");
    // Anything else (including the ones Word reads and PowerPoint does not) is the fallback.
    for p in [
        "w", "wps", "w14", "pic", "v", "o", "p14", "cx1", "p15", "A", "a14x", "ax",
    ] {
        assert_eq!(px::body1(&ac_slide(p)), "fallback", "shape level, {p}");
        assert_eq!(
            px::body1(&px::tb(0, 0, I, I, &ac_inline(p))),
            "x fallback",
            "inline, {p}"
        );
    }
    // One namespace it does not read spoils the others, wherever it stands.
    for r in ["a14 p14", "p14 a14", "a p14 m", "a  p14", "a14\tp14"] {
        assert_eq!(px::body1(&ac_slide(r)), "fallback", "{r:?}");
    }
    // Several spaces and tabs between the names are fine; a blank `Requires` requires nothing.
    assert_eq!(px::body1(&ac_slide("  a14   m ")), "choice");
    assert_eq!(px::body1(&ac_slide("")), "choice");
}

#[test]
fn a_document_reads_each_namespace_it_names_in_a_choice() {
    let ac = |requires: &str| {
        format!(
            r#"<mc:AlternateContent><mc:Choice Requires="{requires}">{}</mc:Choice><mc:Fallback>{}</mc:Fallback></mc:AlternateContent>"#,
            dx::p("choice"),
            dx::p("fallback")
        )
    };
    for r in DOCX_READS_NOW {
        assert_eq!(dx::md(&ac(r)), "choice", "{r}");
    }
    assert_eq!(dx::md(&ac("wps wpg w14 a14")), "choice");
    // The ones PowerPoint reads and Word does not (chart parts and the like) are the fallback.
    for r in [
        "p", "dgm", "dsp", "c", "cx1", "cx", "p14", "wne", "w", "x w",
    ] {
        let want = if r == "w" { "choice" } else { "fallback" };
        assert_eq!(dx::md(&ac(r)), want, "{r}");
    }
    for r in ["wps cx1", "cx1 wps", "w14 p14"] {
        assert_eq!(dx::md(&ac(r)), "fallback", "{r}");
    }
}

#[test]
fn a_document_embedded_object_name_is_cut_at_80_characters() {
    let obj = |id: &str| {
        let o = format!(
            r#"<w:r><w:object><o:OLEObject Type="Embed" ProgID="{id}" r:id="rId9"/></w:object></w:r>"#
        );
        dx::md(&dx::para(&o))
    };
    for (n, want) in [(80, 80), (81, 80), (79, 79)] {
        let m = obj(&"q".repeat(n));
        assert_eq!(m.matches('q').count(), want, "{n}: {m}");
    }
}

// ===============================================================================================
// OpenDocument presentations
// ===============================================================================================

fn odp_md(shapes: &str) -> String {
    od::md1(shapes)
}

/// A frame with an optional class, anywhere, holding `text` in a text box.
fn fr_text(class: &str, x: f64, y: f64, w: f64, h: f64, text: &str) -> String {
    od::fr(class, x, y, w, h, &od::tbx(&od::tp(text)))
}

// The scene for the shapes that hold nothing: "alfa" (left, lower) and "bravo" (right, upper) are
// two columns (their gap is wider than the stripe gap). A shape spanning the gap, at the top, ties
// them into one block and the order becomes rows (bravo first); a shape that is skipped for
// holding nothing leaves them as columns (alfa first).
fn bridge(w: &str) -> String {
    od::tf(1.0, 3.0, 6.0, 2.0, "alfa") + &od::tf(9.0, 0.5, 6.0, 2.0, "bravo") + w
}

fn bridge_order(w: &str) -> Vec<String> {
    order(&odp_md(&bridge(w)), &["alfa", "bravo"])
}

#[test]
fn the_bridge_scene_tells_a_shape_that_counts_from_one_that_does_not() {
    assert_eq!(bridge_order(""), ["alfa", "bravo"]);
    assert_eq!(
        bridge_order(&fr_text("", 6.0, 0.0, 4.0, 1.0, "wee")),
        ["bravo", "alfa"]
    );
}

#[test]
fn a_frame_with_nothing_to_show_takes_no_part_in_the_order() {
    for (what, w) in [
        ("an empty text box", od::fr("", 6.0, 0.0, 4.0, 1.0, &od::tbx(""))),
        (
            "only white space",
            od::fr("", 6.0, 0.0, 4.0, 1.0, &od::tbx(&od::tp("   "))),
        ),
        (
            "only a comment",
            od::fr(
                "",
                6.0,
                0.0,
                4.0,
                1.0,
                &od::tbx("<text:p><office:annotation><text:p>comment text</text:p></office:annotation></text:p>"),
            ),
        ),
        (
            "a page thumbnail holding text",
            od::fr(
                "",
                6.0,
                0.0,
                4.0,
                1.0,
                "<draw:page-thumbnail>stray text</draw:page-thumbnail>",
            ),
        ),
        (
            "only a description",
            od::fr(
                "",
                6.0,
                0.0,
                4.0,
                1.0,
                &(od::tbx("") + "<svg:title>alt title</svg:title><svg:desc>alt text</svg:desc>"),
            ),
        ),
    ] {
        assert_eq!(bridge_order(&w), ["alfa", "bravo"], "{what}");
    }
    // The same for a drawing shape: an empty one is skipped, one with text counts.
    let empty = r#"<draw:custom-shape draw:layer="layout" svg:width="4cm" svg:height="1cm" svg:x="6cm" svg:y="0cm"></draw:custom-shape>"#;
    assert_eq!(bridge_order(empty), ["alfa", "bravo"]);
    let full = r#"<draw:custom-shape draw:layer="layout" svg:width="4cm" svg:height="1cm" svg:x="6cm" svg:y="0cm"><text:p>wee</text:p></draw:custom-shape>"#;
    assert_eq!(bridge_order(full), ["bravo", "alfa"]);
}

#[test]
fn things_that_are_content_count_even_if_they_hold_no_text() {
    // A picture, an object and a table with empty cells are content.
    let image = od::fr(
        "",
        6.0,
        0.0,
        4.0,
        1.0,
        r#"<draw:image xlink:href="Pictures/none.png"/>"#,
    );
    let object = od::fr(
        "",
        6.0,
        0.0,
        4.0,
        1.0,
        r#"<draw:object xlink:href="./Object 1"/>"#,
    );
    let ole = od::fr(
        "",
        6.0,
        0.0,
        4.0,
        1.0,
        r#"<draw:object-ole xlink:href="./Object 1"/>"#,
    );
    let table = od::fr(
        "",
        6.0,
        0.0,
        4.0,
        1.0,
        "<table:table><table:table-column/><table:table-row><table:table-cell><text:p/></table:table-cell></table:table-row></table:table>",
    );
    for (what, w) in [
        ("image", image),
        ("object", object),
        ("object-ole", ole),
        ("table", table),
    ] {
        assert_eq!(bridge_order(&w), ["bravo", "alfa"], "{what}");
    }
}

#[test]
fn a_text_field_is_content_but_a_description_is_not() {
    // `text:title` is a field with the stored text of the document's title: it is shown.
    let f = od::fr(
        "",
        1.0,
        1.0,
        8.0,
        1.0,
        &od::tbx("<text:p><text:title>Deck Title Field</text:title></text:p>"),
    );
    assert_eq!(odp_md(&f), "## Slide 1\n\nDeck Title Field");
    assert_eq!(
        bridge_order(&od::fr(
            "",
            6.0,
            0.0,
            4.0,
            1.0,
            &od::tbx("<text:p><text:title>x</text:title></text:p>")
        )),
        ["bravo", "alfa"]
    );
}

#[test]
fn the_frames_of_a_slide_that_are_furniture_are_not_shown_in_any_shape() {
    for class in ["header", "footer", "date-time", "page-number"] {
        let frame = fr_text(class, 1.0, 1.0, 5.0, 1.0, "FRAMETEXT");
        let shape = format!(
            r#"<draw:custom-shape draw:layer="layout" presentation:class="{class}" svg:width="5cm" svg:height="1cm" svg:x="1cm" svg:y="3cm"><text:p>SHAPETEXT</text:p></draw:custom-shape>"#
        );
        assert_eq!(odp_md(&(frame + &shape)), "## Slide 1", "{class}");
    }
}

#[test]
fn a_hyperlink_around_shapes_is_the_shapes_themselves() {
    // Left "xray" and right "zulu" are wrapped in a link; "mike" is between. Read as the shapes
    // they are (a row) the order is xray, mike, zulu. Taken as a group around xray and zulu it
    // would hold mike (underlay) and the order would be xray, zulu, mike.
    let link = format!(
        r#"<draw:a xlink:href="https://example.com/" xlink:type="simple">{}{}</draw:a>"#,
        od::tf(1.0, 1.0, 4.0, 1.0, "xray"),
        od::tf(21.0, 1.0, 4.0, 1.0, "zulu")
    );
    let s = link + &od::tf(10.0, 1.0, 4.0, 1.0, "mike");
    assert_eq!(
        order(&odp_md(&s), &["xray", "mike", "zulu"]),
        ["xray", "mike", "zulu"]
    );
    // A real group is one unit: its members are read together.
    let g = format!(
        "<draw:g>{}{}</draw:g>",
        od::tf(1.0, 1.0, 4.0, 1.0, "xray"),
        od::tf(21.0, 1.0, 4.0, 1.0, "zulu")
    );
    let s = g + &od::tf(10.0, 1.0, 4.0, 1.0, "mike");
    assert_eq!(
        order(&odp_md(&s), &["xray", "mike", "zulu"]),
        ["xray", "zulu", "mike"]
    );
}

fn title_frame(inner: &str) -> String {
    od::fr("title", 1.0, 0.5, 20.0, 2.0, inner)
}

#[test]
fn the_title_of_a_slide_is_plain_text_without_notes_and_descriptions() {
    // A tab is a space.
    let t = title_frame(&od::tbx("<text:p>A<text:tab/>B</text:p>"));
    assert_eq!(odp_md(&t), "## Slide 1: A B");
    // A footnote's text is not the title's.
    let t = title_frame(&od::tbx(
        r#"<text:p>Hello<text:note text:id="ftn1" text:note-class="footnote"><text:note-citation>1</text:note-citation><text:note-body><text:p>footnote body</text:p></text:note-body></text:note></text:p>"#,
    ));
    let m = odp_md(&t);
    assert!(m.starts_with("## Slide 1: Hello"), "{m}");
    assert!(!m.lines().next().unwrap().contains("footnote body"), "{m}");
    // The description of the frame is not the title's.
    let t = title_frame(
        &(od::tbx(&od::tp("Real"))
            + "<svg:title>Alt text</svg:title><svg:desc>Longer alt</svg:desc>"),
    );
    assert_eq!(odp_md(&t), "## Slide 1: Real");
    // Line breaks and spaces separate words.
    let t = title_frame(&od::tbx(
        "<text:p>one<text:line-break/>two<text:s text:c=\"3\"/>three</text:p>",
    ));
    assert_eq!(odp_md(&t), "## Slide 1: one two three");
}

#[test]
fn a_title_is_cut_at_200_characters() {
    for (n, want) in [(200, 200), (201, 200), (199, 199)] {
        let d = od::doc(&od::Op::new(&od::page(&title_frame(&od::tbx(&od::tp(
            &"t".repeat(n),
        ))))));
        assert_eq!(d.slides[0].title.chars().count(), want, "{n}");
    }
}

#[test]
fn a_transform_is_followed_for_sixteen_operations_and_not_for_seventeen() {
    // "kay" is 2cm wide at x=1cm and moved right by one centimetre per operation. With 16 it ends
    // at 17cm, right of "cee" (at 10cm); a transform of 17 operations is not understood and the
    // shape stays where it says (1cm), left of it.
    let kay = |ops: usize| {
        format!(
            r#"<draw:frame draw:layer="layout" svg:width="2cm" svg:height="2cm" svg:x="1cm" svg:y="0cm" draw:transform="{}"><draw:text-box><text:p>kay</text:p></draw:text-box></draw:frame>"#,
            "translate (1cm 0cm) ".repeat(ops)
        )
    };
    let s = |ops: usize| kay(ops) + &od::tf(10.0, 0.0, 2.0, 2.0, "cee");
    assert_eq!(order(&odp_md(&s(16)), &["kay", "cee"]), ["cee", "kay"]);
    assert_eq!(order(&odp_md(&s(17)), &["kay", "cee"]), ["kay", "cee"]);
    assert_eq!(order(&odp_md(&s(1)), &["kay", "cee"]), ["kay", "cee"]);
}

// --- master pages, page layouts ---------------------------------------------------------------

const PAGE_LAYOUT: &str = r#"<style:page-layout style:name="PM1"><style:page-layout-properties fo:page-width="28cm" fo:page-height="15.75cm"/></style:page-layout>"#;

fn sized(s: &str) -> od::Op {
    od::Op::new(&od::page_with("dp1", "Default", s))
        .styles_auto(PAGE_LAYOUT)
        .master(r#"<style:master-page style:name="Default" style:page-layout-name="PM1"/>"#)
}

#[test]
fn the_page_size_is_width_by_height() {
    // 28 x 15.75cm. "pee" at x=26..28 is on the page; "kew" (x 15..28, below) overlaps it in x:
    // a stripe, pee first. Sides swapped (15.75 x 28): pee off the page, read last.
    let s = od::tf(26.0, 0.0, 2.0, 1.0, "pee") + &od::tf(15.0, 4.0, 13.0, 1.0, "kew");
    assert_eq!(
        order(&od::doc(&sized(&s)).markdown, &["pee", "kew"]),
        ["pee", "kew"]
    );
    // "vee" at y=20 is below the 15.75 page: after "why", which is on it.
    let s = od::tf(1.0, 20.0, 2.0, 1.0, "vee") + &od::tf(10.0, 15.0, 2.0, 1.0, "why");
    assert_eq!(
        order(&od::doc(&sized(&s)).markdown, &["vee", "why"]),
        ["why", "vee"]
    );
}

#[test]
fn a_page_layout_with_one_of_the_two_sizes_gives_no_size() {
    let s = od::tf(-200.0, 1.0, 5.0, 1.0, "far") + &od::tf(1.0, 1.0, 5.0, 1.0, "near");
    for layout in [
        r#"<style:page-layout style:name="PM1"><style:page-layout-properties fo:page-width="28cm"/></style:page-layout>"#,
        r#"<style:page-layout style:name="PM1"><style:page-layout-properties fo:page-height="15cm"/></style:page-layout>"#,
        r#"<style:page-layout style:name="PM1"><style:page-layout-properties/></style:page-layout>"#,
    ] {
        let o = od::Op::new(&od::page_with("dp1", "Default", &s))
            .styles_auto(layout)
            .master(r#"<style:master-page style:name="Default" style:page-layout-name="PM1"/>"#);
        // Position order: "far" first.
        assert_eq!(
            order(&od::doc(&o).markdown, &["far", "near"]),
            ["far", "near"],
            "{layout}"
        );
    }
    let o = sized(&s);
    assert_eq!(
        order(&od::doc(&o).markdown, &["far", "near"]),
        ["near", "far"]
    );
}

#[test]
fn a_master_page_shape_that_is_not_a_frame_gives_its_class_the_position() {
    // The master's outline is a custom shape (not a frame): the slide's outline still takes it.
    let m = r#"<style:master-page style:name="Default" style:page-layout-name="PM1"><draw:custom-shape draw:layer="layout" presentation:class="outline" svg:width="5cm" svg:height="1cm" svg:x="1cm" svg:y="2cm"/></style:master-page>"#;
    let s =
        od::tf(1.0, 9.0, 5.0, 1.0, "placed") + &od::fr_nopos("outline", &od::tbx(&od::tp("late")));
    let o = od::Op::new(&od::page_with("dp1", "Default", &s)).master(m);
    assert_eq!(
        order(&od::doc(&o).markdown, &["late", "placed"]),
        ["late", "placed"]
    );
}

fn many_masters(n: usize) -> od::Op {
    // Master `Mi` has an outline at the top and the page layout `PL{n-1-i}`: so `M0` has the
    // layout that comes last in the file.
    let masters: String = (0..n)
        .map(|i| {
            format!(
                r#"<style:master-page style:name="M{i}" style:page-layout-name="PL{}"><draw:frame draw:layer="layout" presentation:class="outline" svg:width="5cm" svg:height="1cm" svg:x="1cm" svg:y="1cm"><draw:text-box/></draw:frame></style:master-page>"#,
                n - 1 - i
            )
        })
        .collect();
    let layouts: String = (0..n)
        .map(|i| {
            format!(
                r#"<style:page-layout style:name="PL{i}"><style:page-layout-properties fo:page-width="28cm" fo:page-height="15.75cm"/></style:page-layout>"#
            )
        })
        .collect();
    od::Op::new("").master(&masters).styles_auto(&layouts)
}

/// The deck: one slide on `master`, with an outline that has no position, a box at 9cm, and one
/// far to the left.
fn on_master(o: od::Op, master: &str) -> (Vec<String>, Vec<String>) {
    let s = od::tf(1.0, 9.0, 5.0, 1.0, "placed")
        + &od::fr_nopos("outline", &od::tbx(&od::tp("late")))
        + &od::tf(-200.0, 12.0, 5.0, 1.0, "far");
    let mut o = o;
    o.pages = od::page_with("dp1", master, &s);
    let md = od::doc(&o).markdown;
    (
        order(&md, &["late", "placed"]),
        order(&md, &["far", "placed"]),
    )
}

#[test]
fn a_thousand_master_pages_and_a_thousand_page_layouts_are_read_and_no_more() {
    // 1,000 masters (and 1,000 layouts) are kept.
    let (frames, size) = on_master(many_masters(1000), "M999");
    assert_eq!(
        frames,
        ["late", "placed"],
        "the 1000th master has its frames"
    );
    assert_eq!(
        size,
        ["placed", "far"],
        "... and its page size (layout PL0)"
    );
    // A 1001st master is not read: no frames, no size.
    let (frames, size) = on_master(many_masters(1001), "M1000");
    assert_eq!(frames, ["placed", "late"]);
    assert_eq!(size, ["far", "placed"]);
    // 1,001 layouts: the one that comes last is not read, so the master that uses it (M0) has its
    // frames but no size.
    let (frames, size) = on_master(many_masters(1001), "M0");
    assert_eq!(frames, ["late", "placed"]);
    assert_eq!(size, ["far", "placed"]);
    // ... and M1 (layout PL999, the 1000th) has a size.
    let (_, size) = on_master(many_masters(1001), "M1");
    assert_eq!(size, ["placed", "far"]);
}

// --- fields, lists, notes, tables, charts -----------------------------------------------------

#[test]
fn a_page_number_field_keeps_the_space_after_it_and_other_elements_called_that_are_text() {
    let t = "<text:p>Slide <text:page-number text:select-page=\"current\">&lt;number&gt;</text:page-number> of nine</text:p>";
    let s = od::fr("", 1.0, 1.0, 8.0, 1.0, &od::tbx(t));
    let o = od::Op::new(&(od::page(&s) + &od::page(&s)));
    assert_eq!(
        od::doc(&o).markdown,
        "## Slide 1\n\nSlide 1 of nine\n\n## Slide 2\n\nSlide 2 of nine"
    );
    // A page number in bold is bold.
    let t = "<text:p><text:span text:style-name=\"B\"><text:page-number>&lt;number&gt;</text:page-number></text:span></text:p>";
    let s = od::fr("", 1.0, 1.0, 8.0, 1.0, &od::tbx(t));
    let o = od::Op::new(&od::page(&s))
        .auto(r#"<style:style style:name="B" style:family="text"><style:text-properties fo:font-weight="bold"/></style:style>"#);
    assert_eq!(od::doc(&o).markdown, "## Slide 1\n\n**1**");
    // An element that only has the name (another namespace) is not the field.
    let t = "<text:p>x<presentation:page-number>stored</presentation:page-number></text:p>";
    let s = od::fr("", 1.0, 1.0, 8.0, 1.0, &od::tbx(t));
    assert_eq!(odp_md(&s), "## Slide 1\n\nxstored");
}

#[test]
fn the_notes_of_a_slide_are_flat_text_not_lists() {
    let list = "<text:list><text:list-item><text:p>one</text:p></text:list-item><text:list-item><text:p>two</text:p></text:list-item></text:list>";
    let nf = od::fr("notes", 2.0, 13.0, 17.0, 12.0, &od::tbx(list));
    let s = od::title("T") + &od::notes(&nf);
    let m = odp_md(&s);
    // (The list is flattened: its items are bullet characters in one quote, not Markdown items.)
    assert!(m.contains("> \u{2022} one"), "{m}");
    assert!(m.contains("> \u{2022} two"), "{m}");
    assert!(!m.contains("- one") && !m.contains("> - "), "{m}");
}

fn nested_tables(n: usize) -> String {
    let mut inner = "<text:p>deeptext</text:p>".to_string();
    for _ in 0..n {
        inner = format!(
            "<table:table><table:table-column/><table:table-row><table:table-cell>{inner}</table:table-cell></table:table-row></table:table>"
        );
    }
    inner
}

#[test]
fn tables_in_tables_are_followed_to_a_depth() {
    let shown =
        |n: usize| odp_md(&od::fr("", 1.0, 1.0, 10.0, 5.0, &nested_tables(n))).contains("deeptext");
    let deepest = (1..20).take_while(|&n| shown(n)).last().unwrap();
    assert_eq!(deepest, 12);
}

#[test]
fn groups_are_followed_twenty_deep_in_a_drawing_too() {
    let nest = |n: usize| {
        let mut s = od::tf(1.0, 1.0, 5.0, 1.0, "deep");
        for _ in 0..n {
            s = format!("<draw:g>{s}</draw:g>");
        }
        s
    };
    let at = od::doc(&od::Op::new(&od::page(&nest(20))));
    assert!(at.markdown.contains("deep"), "{}", at.markdown);
    assert!(!at.truncated);
    let past = od::doc(&od::Op::new(&od::page(&nest(21))));
    assert!(!past.markdown.contains("deep"), "{}", past.markdown);
    assert!(past.truncated);
}

fn object_frame(href: &str, x: f64, y: f64) -> String {
    od::fr(
        "",
        x,
        y,
        6.0,
        2.0,
        &format!(r#"<draw:object xlink:href="{href}" xlink:type="simple"/>"#),
    )
}

#[test]
fn a_formula_beside_text_stays_inline_and_one_beside_a_text_box_is_not_a_display() {
    let mml = r#"<?xml version="1.0" encoding="UTF-8"?><math xmlns="http://www.w3.org/1998/Math/MathML"><semantics><mrow><mi>E</mi></mrow><annotation encoding="StarMath 5.0">E</annotation></semantics></math>"#;
    // A frame with the formula and a caption in a text box of its own: the formula is not alone.
    let f = od::fr(
        "",
        1.0,
        1.0,
        6.0,
        2.0,
        &format!(
            r#"<draw:object xlink:href="./Object 1" xlink:type="simple"/>{}"#,
            od::tbx(&od::tp("caption"))
        ),
    );
    let m = od::doc(&od::Op::new(&od::page(&f)).object("Object 1", mml)).markdown;
    assert!(m.contains("caption"), "{m}");
    assert!(!m.contains("$$"), "a formula with a caption is inline: {m}");
    // A frame that is only the formula is a display formula.
    let m = od::doc(
        &od::Op::new(&od::page(&object_frame("./Object 1", 1.0, 1.0))).object("Object 1", mml),
    )
    .markdown;
    assert!(m.contains("$$"), "{m}");
    // A formula inside a paragraph of a text box is part of its line.
    let t = r#"<text:p>Let <draw:frame svg:width="1cm" svg:height="1cm"><draw:object xlink:href="./Object 1" xlink:type="simple"/></draw:frame> be it</text:p>"#.to_string();
    let m = od::doc(
        &od::Op::new(&od::page(&od::fr("", 1.0, 1.0, 8.0, 1.0, &od::tbx(&t))))
            .object("Object 1", mml),
    )
    .markdown;
    assert!(
        m.contains("Let $") && m.contains("be it") && !m.contains("$$"),
        "{m}"
    );
}

#[test]
fn objects_that_are_not_charts_use_up_the_chart_read_budget() {
    // Each object read costs at least 1 KiB of the 4 MiB budget, whatever its size: 4,096 of them
    // use it up and a chart after them is no longer looked into. The objects are one tiny
    // document, referenced over and over.
    let not_chart = format!(
        r#"<office:document-content {}><office:body><office:text><text:p>x</text:p></office:text></office:body></office:document-content>"#,
        od::pns()
    );
    let chart = format!(
        r#"<office:document-content {}><office:body><office:chart><chart:chart chart:class="chart:bar"><chart:title><text:p>Sales</text:p></chart:title></chart:chart></office:chart></office:body></office:document-content>"#,
        od::pns()
    );
    let frame = |y: f64, href: &str| {
        od::fr(
            "",
            1.0,
            y,
            2.0,
            0.01,
            &format!(r#"<draw:object xlink:href="{href}" xlink:type="simple"/>"#),
        )
    };
    let opts = opts_with(|o| {
        o.max_slide_shapes = 100_000;
        o.max_block_nodes = 1_000_000;
        o.max_block_text_bytes = 64 * 1024 * 1024;
        o.max_markdown_bytes = 64 * 1024 * 1024;
        o.max_markdown_lines = 1_000_000;
    });
    // The chart comes last in the file and in the reading order, after the budget.
    let mut s: String = (0..4200)
        .map(|i| frame(1.0 + i as f64 * 0.001, "./Object 1"))
        .collect();
    s += &frame(9.0, "./Object 2");
    let o = od::Op::new(&od::page(&s))
        .object("Object 1", &not_chart)
        .object("Object 2", &chart);
    let dir = tmp("odpchart2");
    let p = write(&dir, "t.odp", &o.bytes());
    let md = load_presentation(&p, &opts).unwrap().markdown;
    assert!(!md.contains("chart: Sales"), "the budget is used up");
    // With few objects the chart is found.
    let mut s: String = (0..10)
        .map(|i| frame(1.0 + i as f64 * 0.001, "./Object 1"))
        .collect();
    s += &frame(9.0, "./Object 2");
    let o = od::Op::new(&od::page(&s))
        .object("Object 1", &not_chart)
        .object("Object 2", &chart);
    let p = write(&dir, "t2.odp", &o.bytes());
    let md = load_presentation(&p, &opts).unwrap().markdown;
    assert!(
        md.contains("chart: Sales"),
        "{}",
        &md[md.len().saturating_sub(300)..]
    );
}

#[test]
fn the_slides_of_an_odp_are_read_while_the_bytes_read_so_far_stay_within_the_budget() {
    // `max_pptx_read_total` also bounds how far into `content.xml` the slides are read: a slide is
    // read when the position after its start tag is within the budget.
    let pages: String = (1..=4)
        .map(|i| od::page(&od::title(&format!("T{i}"))))
        .collect();
    let o = od::Op::new(&pages);
    let content = o.content_xml();
    // The position just after the start tag of each page.
    let mut ends = Vec::new();
    let mut from = 0;
    while let Some(at) = content[from..].find("<draw:page ") {
        let start = from + at;
        let end = start + content[start..].find('>').unwrap() + 1;
        ends.push(end as u64);
        from = end;
    }
    assert_eq!(ends.len(), 4);
    let dir = tmp("odpbudget");
    let p = write(&dir, "t.odp", &o.bytes());
    for (k, end) in ends.iter().enumerate() {
        let opts = opts_with(|o| o.max_pptx_read_total = *end);
        let d = load_presentation(&p, &opts).unwrap();
        assert_eq!(d.slides.len(), k + 1, "budget {end}: {}", d.markdown);
        assert_eq!(d.truncated, k + 1 < 4, "budget {end}");
        // One byte less loses that slide.
        let opts = opts_with(|o| o.max_pptx_read_total = *end - 1);
        let d = load_presentation(&p, &opts).unwrap();
        assert_eq!(d.slides.len(), k, "budget {}", end - 1);
        assert!(d.truncated);
    }
}

// ===============================================================================================
// second batch: links, hidden slides, the relationships cap, the cancel hook, notes frames
// ===============================================================================================

#[test]
fn a_link_that_ends_a_paragraph_or_a_line_is_closed() {
    let link = |t: &str| {
        format!(
            r#"<a:r><a:rPr lang="en-US"><a:hlinkClick r:id="rIdL"/></a:rPr><a:t>{t}</a:t></a:r>"#
        )
    };
    let deck = |tx: &str| {
        px::Px::new(vec![px::sl(&px::tb(0, 0, I, I, tx)).rel(
            "rIdL",
            "hyperlink",
            "https://example.com/",
            true,
        )])
    };
    let last = px::paras(&format!("{}{}", px::ru("", "see "), link("link")));
    assert_eq!(
        px::doc(&deck(&last)).markdown,
        "## Slide 1\n\nsee [link](https://example.com/)"
    );
    let mid = px::paras(&format!("{}<a:br/>{}", link("link"), px::ru("", "next")));
    assert_eq!(
        px::doc(&deck(&mid)).markdown,
        "## Slide 1\n\n[link](https://example.com/)  \nnext"
    );
    // In a table cell too.
    let cell = format!(
        r#"<a:tr h="1"><a:tc><a:txBody><a:bodyPr/><a:lstStyle/>{}</a:txBody><a:tcPr/></a:tc></a:tr>"#,
        px::paras(&link("celllink"))
    );
    let d = px::doc(&px::Px::new(vec![px::sl(&px::table(
        (0, 0, 2 * I, I),
        &cell,
    ))
    .rel("rIdL", "hyperlink", "https://example.com/", true)]));
    assert!(
        d.markdown.contains("[celllink](https://example.com/)"),
        "{}",
        d.markdown
    );
}

#[test]
fn a_slide_is_hidden_when_show_says_zero_or_false() {
    for (show, hidden) in [
        ("0", true),
        ("false", true),
        (" 0 ", true),
        ("1", false),
        ("true", false),
        ("", false),
    ] {
        let d = px::doc(&px::Px::new(vec![px::sl(&px::title("T")).show(show)]));
        assert_eq!(d.slides[0].hidden, hidden, "show={show:?}");
        assert_eq!(d.markdown.contains("(hidden)"), hidden, "show={show:?}");
    }
}

#[test]
fn a_relationships_part_is_read_up_to_four_mebibytes() {
    const CAP: usize = 4 * 1024 * 1024;
    let pic = px::pic("rId5", "photo", Some((0, 0, 2 * I, 2 * I)));
    let deck = || {
        px::Px::new(vec![px::sl(&pic).rel(
            "rId5",
            "image",
            "../media/f.png",
            false,
        )])
        .media("f.png", super::tests_docx::tiny_png(1))
    };
    let rels = "ppt/slides/_rels/slide1.xml.rels";
    let run = |extra: isize| {
        load_edited(&deck(), &DocOptions::default(), |e| {
            let len = part_len(e, rels) as usize;
            // "<!--" + padding + "-->" is 7 bytes of its own.
            let pad = (CAP as isize + extra) as usize - len - 7;
            edit_part(e, rels, |s| {
                s.replace(
                    "</Relationships>",
                    &format!("<!--{}--></Relationships>", "r".repeat(pad)),
                )
            });
            assert_eq!(part_len(e, rels) as isize, CAP as isize + extra);
        })
        .unwrap()
    };
    let at = run(0);
    assert!(
        at.markdown.contains("![photo](office-img://"),
        "{}",
        at.markdown
    );
    assert!(!at.truncated);
    let over = run(1);
    assert!(
        !over.markdown.contains("office-img://"),
        "{}",
        over.markdown
    );
    assert!(over.truncated);
}

#[test]
fn the_cancel_hook_is_asked_before_every_part_and_every_shape() {
    // One slide of N shapes with notes: asked once for the slide, once before each of the seven
    // parts read (slide, its relationships, layout, its relationships, master, notes, notes
    // relationships) and once before each shape.
    for n in [1usize, 5, 12] {
        let boxes: String = (0..n)
            .map(|i| px::tb(0, i as i64 * (I / 4), I, I / 8, &px::pa(&format!("b{i}"))))
            .collect();
        let p = px::Px::new(vec![px::sl(&boxes).notes(&px::sp_raw(
            3,
            r#"<p:ph type="body" idx="1"/>"#,
            "",
            &px::pa("note"),
        ))]);
        let dir = tmp("pptxasked");
        let path = write(&dir, "t.pptx", &p.bytes());
        let (c, count) = counting_cancel(usize::MAX);
        let d = load_presentation_cancellable(&path, &DocOptions::default(), Some(&c)).unwrap();
        assert!(!d.truncated && d.markdown.contains("note"));
        let asked = count.load(Ordering::Relaxed);
        assert!(asked >= 1 + 7 + n, "n={n}: asked {asked} times");
        // Cancelled at any one of the questions, the result says it is not whole.
        for k in 0..asked {
            let n2 = Arc::new(AtomicUsize::new(0));
            let n3 = n2.clone();
            let c = Cancel::new(move || n3.fetch_add(1, Ordering::Relaxed) == k);
            let d = load_presentation_cancellable(&path, &DocOptions::default(), Some(&c)).unwrap();
            assert!(d.truncated, "n={n}, cancelled at question {k}");
        }
    }
}

#[test]
fn the_notes_page_of_an_odp_has_eight_frames_read() {
    let frames = |n: usize| -> String {
        (1..=n)
            .map(|i| {
                od::fr(
                    "notes",
                    2.0,
                    13.0,
                    17.0,
                    12.0,
                    &od::tbx(&od::tp(&format!("nf{i}"))),
                )
            })
            .collect()
    };
    let s = |n: usize| od::title("T") + &od::notes(&frames(n));
    let m8 = odp_md(&s(8));
    for i in 1..=8 {
        assert!(m8.contains(&format!("nf{i}")), "{m8}");
    }
    let m9 = odp_md(&s(9));
    assert!(m9.contains("nf8") && !m9.contains("nf9"), "{m9}");
}

#[test]
fn a_page_number_in_a_paragraph_with_a_formula_leaves_the_formula_inline() {
    let mml = r#"<?xml version="1.0" encoding="UTF-8"?><math xmlns="http://www.w3.org/1998/Math/MathML"><semantics><mrow><mi>E</mi></mrow><annotation encoding="StarMath 5.0">E</annotation></semantics></math>"#;
    let t = r#"<text:p><text:page-number>&lt;n&gt;</text:page-number><draw:frame svg:width="1cm" svg:height="1cm"><draw:object xlink:href="./Object 1" xlink:type="simple"/></draw:frame></text:p>"#;
    let o = od::Op::new(&od::page(&od::fr("", 1.0, 1.0, 8.0, 1.0, &od::tbx(t))))
        .object("Object 1", mml);
    let m = od::doc(&o).markdown;
    assert_eq!(m, "## Slide 1\n\n1$E$");
}

#[test]
fn a_second_title_is_text_after_the_subtitles_in_the_order_of_the_file() {
    // Title A is the heading; the subtitle and the second title are text, in the order of the file.
    let s = [
        px::title("Atitle"),
        px::ph(
            r#"type="subTitle" idx="1""#,
            Some((0, 0, I, I)),
            &px::pa("subtext"),
        ),
        px::title("Btitle"),
    ]
    .concat();
    let m = px::md1(&s);
    assert!(m.starts_with("## Slide 1: Atitle"), "{m}");
    assert_eq!(order(&m, &["subtext", "Btitle"]), ["subtext", "Btitle"]);
    assert!(!m.contains("Atitle\n\nAtitle"), "{m}");
    assert_eq!(m.matches("Atitle").count(), 1, "{m}");
}

#[test]
fn a_hyperlink_with_no_text_shows_nothing_and_a_leading_space_stays_outside_a_link() {
    let empty_link =
        r#"<a:r><a:rPr lang="en-US"><a:hlinkClick r:id="rIdL"/></a:rPr><a:t></a:t></a:r>"#;
    let deck = |tx: &str| {
        px::Px::new(vec![px::sl(&px::tb(0, 0, I, I, tx)).rel(
            "rIdL",
            "hyperlink",
            "https://example.com/",
            true,
        )])
    };
    // A paragraph that is only an empty link is not a paragraph.
    let tx = px::paras(empty_link) + &px::pa("kept");
    assert_eq!(px::doc(&deck(&tx)).markdown, "## Slide 1\n\nkept");
    // ... nor in the speaker notes.
    let p = px::Px::new(vec![px::sl(&px::title("T"))
        .notes(&px::sp_raw(
            3,
            r#"<p:ph type="body" idx="1"/>"#,
            "",
            &(px::paras(empty_link) + &px::pa("note")),
        ))
        .rel("rIdL", "hyperlink", "https://example.com/", true)]);
    let m = px::doc(&p).markdown;
    assert!(!m.contains("]("), "{m}");
    assert!(m.contains("> note"), "{m}");
    // The white space at the start of a link's text is outside the link, also when the link ends
    // the paragraph.
    let link =
        r#"<a:r><a:rPr lang="en-US"><a:hlinkClick r:id="rIdL"/></a:rPr><a:t> link</a:t></a:r>"#;
    let tx = px::paras(&format!("{}{}", px::ru("", "see"), link));
    assert_eq!(
        px::doc(&deck(&tx)).markdown,
        "## Slide 1\n\nsee [link](https://example.com/)"
    );
}

#[test]
fn a_slide_listed_again_right_after_itself_is_not_read_again() {
    // The read budget counts every part read: five references to one slide cost the parts of one
    // read (the slide and its relationships; the layout and the master are kept by name anyway).
    let p = px::Px::new(vec![px::sl(&px::tb(0, 0, I, I, &px::pa("hello")))]);
    let mut e = p.entries();
    edit_part(&mut e, "ppt/presentation.xml", |s| {
        s.replace(
            r#"<p:sldId id="256" r:id="rIdS0"/>"#,
            &r#"<p:sldId id="256" r:id="rIdS0"/>"#.repeat(5),
        )
    });
    let total: u64 = [
        "ppt/slides/slide1.xml",
        "ppt/slides/_rels/slide1.xml.rels",
        "ppt/slideLayouts/slideLayout1.xml",
        "ppt/slideLayouts/_rels/slideLayout1.xml.rels",
        "ppt/slideMasters/slideMaster1.xml",
    ]
    .iter()
    .map(|n| part_len(&e, n))
    .sum();
    let at = load_entries(&e, &opts_with(|o| o.max_pptx_read_total = total)).unwrap();
    assert_eq!(at.slides.len(), 5);
    assert_eq!(at.markdown.matches("hello").count(), 5, "{}", at.markdown);
    assert!(!at.truncated);
}

#[test]
fn a_notes_paragraph_that_is_only_a_line_break_is_not_a_line() {
    let p = px::Px::new(vec![px::sl(&px::title("T")).notes(&px::sp_raw(
        3,
        r#"<p:ph type="body" idx="1"/>"#,
        "",
        &(px::paras("<a:br/>") + &px::pa("note") + &px::paras("<a:br/>")),
    ))]);
    assert_eq!(
        px::doc(&p).markdown,
        "## Slide 1: T\n\n> **Notes**  \n> note"
    );
}
