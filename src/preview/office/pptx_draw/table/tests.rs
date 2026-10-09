//! Tests of the table drawing: the grid and its merges, row growth, every `tcPr` property, the
//! table style and its precedence, shared borders, budgets and hostile tables.

use super::*;
use crate::preview::office::docx::pptx::pptx_draw::tests::{slide, D};
use crate::preview::office::docx::pptx::{load_presentation, DocOptions};
use crate::preview::office::slide_draw::{Fill, Item, Rgba, ShapeItem, SlideScene};
use crate::preview::office::tests::{deflated, tmp, write};

mod dump;

// ---------------------------------------------------------------------------------------------
// builders
// ---------------------------------------------------------------------------------------------

/// One `a:tc`: attributes of the `tc`, its text (empty: an empty paragraph), the attributes and
/// the children of its `tcPr`. The text is 10 pt.
fn tc(attrs: &str, text: &str, pr_attrs: &str, pr: &str) -> String {
    let p = if text.is_empty() {
        r#"<a:p><a:endParaRPr lang="en-US" sz="1000"/></a:p>"#.to_string()
    } else {
        format!(r#"<a:p><a:r><a:rPr lang="en-US" sz="1000"/><a:t>{text}</a:t></a:r></a:p>"#)
    };
    format!(
        r#"<a:tc {attrs}><a:txBody><a:bodyPr/><a:lstStyle/>{p}</a:txBody><a:tcPr {pr_attrs}>{pr}</a:tcPr></a:tc>"#
    )
}

fn plain(text: &str) -> String {
    tc("", text, "", "")
}

fn tr(h: i64, cells: &[String]) -> String {
    format!(r#"<a:tr h="{h}">{}</a:tr>"#, cells.concat())
}

fn tbl(pr: &str, cols: &[i64], rows: &[String]) -> String {
    let grid: String = cols
        .iter()
        .map(|w| format!(r#"<a:gridCol w="{w}"/>"#))
        .collect();
    format!(
        "<a:tbl>{pr}<a:tblGrid>{grid}</a:tblGrid>{}</a:tbl>",
        rows.concat()
    )
}

fn frame(x: i64, y: i64, w: i64, h: i64, tbl: &str) -> String {
    format!(
        r#"<p:graphicFrame><p:nvGraphicFramePr><p:cNvPr id="4" name="T"/><p:cNvGraphicFramePr/><p:nvPr/></p:nvGraphicFramePr><p:xfrm><a:off x="{x}" y="{y}"/><a:ext cx="{w}" cy="{h}"/></p:xfrm><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/table">{tbl}</a:graphicData></a:graphic></p:graphicFrame>"#
    )
}

fn pr(attrs: &str, style_id: &str) -> String {
    if style_id.is_empty() {
        format!("<a:tblPr {attrs}/>")
    } else {
        format!("<a:tblPr {attrs}><a:tableStyleId>{style_id}</a:tableStyleId></a:tblPr>")
    }
}

const MS2_ACCENT1: &str = "{5C22544A-7EE6-4342-B048-85BDC9FD1C3A}";

/// The scene of a one-slide deck holding `shapes`, with `styles` as `ppt/tableStyles.xml`.
fn scene_with(shapes: &str, styles: Option<&str>, opts: &DocOptions) -> SlideScene {
    let mut d = D::new("");
    d.slides = vec![slide(shapes)];
    let mut e = d.entries();
    if let Some(s) = styles {
        e.push((
            "ppt/tableStyles.xml".into(),
            format!(
                r#"<?xml version="1.0"?><a:tblStyleLst xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" def="{MS2_ACCENT1}">{s}</a:tblStyleLst>"#
            )
            .into_bytes(),
        ));
        for (n, b) in e.iter_mut() {
            if n == "ppt/_rels/presentation.xml.rels" {
                let t = String::from_utf8(b.clone()).unwrap();
                *b = t
                    .replace(
                        "</Relationships>",
                        r#"<Relationship Id="rIdTS" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/tableStyles" Target="tableStyles.xml"/></Relationships>"#,
                    )
                    .into_bytes();
            }
        }
    }
    let dir = tmp("tbl");
    let refs: Vec<(&str, &[u8])> = e.iter().map(|(n, b)| (n.as_str(), b.as_slice())).collect();
    let p = write(&dir, "t.pptx", &deflated(&refs));
    let doc = load_presentation(&p, opts).unwrap();
    doc.slide_scenes[0].clone()
}

fn scene(shapes: &str) -> SlideScene {
    scene_with(shapes, None, &DocOptions::default())
}

fn shapes_of(s: &SlideScene) -> Vec<&ShapeItem> {
    s.items
        .iter()
        .filter_map(|i| match i {
            Item::Shape(sh) => Some(sh),
            _ => None,
        })
        .collect()
}

/// The cell shapes (rectangles), in drawing order.
fn rects(s: &SlideScene) -> Vec<&ShapeItem> {
    shapes_of(s)
        .into_iter()
        .filter(|sh| sh.geom == sd::Geometry::Rect)
        .collect()
}

/// The line shapes.
fn lines(s: &SlideScene) -> Vec<&ShapeItem> {
    shapes_of(s)
        .into_iter()
        .filter(|sh| sh.geom == sd::Geometry::Line)
        .collect()
}

fn hlines(s: &SlideScene) -> Vec<&ShapeItem> {
    lines(s)
        .into_iter()
        .filter(|l| l.xfrm.h == 0.0 && l.xfrm.w > 0.0)
        .collect()
}

fn vlines(s: &SlideScene) -> Vec<&ShapeItem> {
    lines(s)
        .into_iter()
        .filter(|l| l.xfrm.w == 0.0 && l.xfrm.h > 0.0)
        .collect()
}

fn solid(f: &Fill) -> Rgba {
    match f {
        Fill::Solid(c) => *c,
        o => panic!("not solid: {o:?}"),
    }
}

fn line_color(l: &ShapeItem) -> Rgba {
    solid(&l.line.as_ref().unwrap().fill)
}

const RED: &str = r#"<a:solidFill><a:srgbClr val="FF0000"/></a:solidFill>"#;
const BLUE: &str = r#"<a:solidFill><a:srgbClr val="0000FF"/></a:solidFill>"#;

fn ln(tag: &str, w: i64, fill: &str) -> String {
    format!(r#"<a:{tag} w="{w}">{fill}</a:{tag}>"#)
}

// ---------------------------------------------------------------------------------------------
// geometry, merges, growth
// ---------------------------------------------------------------------------------------------

#[test]
fn grid_geometry_and_fills() {
    let t = tbl(
        "<a:tblPr/>",
        &[1_000_000, 2_000_000],
        &[
            tr(500_000, &[tc("", "a", "", RED), tc("", "b", "", BLUE)]),
            tr(600_000, &[plain("c"), plain("d")]),
        ],
    );
    let s = scene(&frame(100_000, 200_000, 3_000_000, 1_100_000, &t));
    let r = rects(&s);
    // (No style: only the two filled cells and the cells with text.)
    assert_eq!(r.len(), 4);
    assert_eq!(
        (r[0].xfrm.x, r[0].xfrm.y, r[0].xfrm.w, r[0].xfrm.h),
        (100_000.0, 200_000.0, 1_000_000.0, 500_000.0)
    );
    assert_eq!(
        (r[1].xfrm.x, r[1].xfrm.w),
        (1_100_000.0, 2_000_000.0),
        "second column"
    );
    assert_eq!((r[2].xfrm.y, r[2].xfrm.h), (700_000.0, 600_000.0));
    assert_eq!(solid(&r[0].fill), Rgba::rgb(255, 0, 0));
    assert_eq!(solid(&r[1].fill), Rgba::rgb(0, 0, 255));
    assert!(!r[2].fill.is_visible());
    assert!(r[2].text.is_some());
    assert!(lines(&s).is_empty(), "no style and no border: no line");
}

#[test]
fn the_grid_not_the_frame_gives_the_size() {
    let t = tbl(
        "<a:tblPr/>",
        &[1_000_000],
        &[tr(400_000, &[tc("", "a", "", RED)])],
    );
    let s = scene(&frame(0, 0, 9_000_000, 9_000_000, &t));
    let r = rects(&s);
    assert_eq!((r[0].xfrm.w, r[0].xfrm.h), (1_000_000.0, 400_000.0));
}

#[test]
fn grid_span_merges_the_cell_and_draws_no_line_inside() {
    let border = ln("lnL", 12700, RED) + &ln("lnR", 12700, RED);
    let t = tbl(
        "<a:tblPr/>",
        &[1_000_000, 1_000_000, 1_000_000],
        &[
            tr(
                400_000,
                &[
                    tc(r#"gridSpan="2""#, "wide", "", &(border.clone() + BLUE)),
                    tc(r#"hMerge="1""#, "", "", &border),
                    plain("x"),
                ],
            ),
            tr(400_000, &[plain("1"), plain("2"), plain("3")]),
        ],
    );
    let s = scene(&frame(0, 0, 3_000_000, 800_000, &t));
    let r = rects(&s);
    assert_eq!(r[0].xfrm.w, 2_000_000.0, "the origin spans both columns");
    assert_eq!(solid(&r[0].fill), Rgba::rgb(0, 0, 255));
    // The hMerge cell draws nothing: the next rectangle is the cell `x`.
    assert_eq!(r[1].xfrm.x, 2_000_000.0);
    // No vertical line at x = 1_000_000 in the first row (inside the merged cell); lines at 0,
    // 2_000_000 do exist.
    let v: Vec<_> = vlines(&s)
        .into_iter()
        .map(|l| (l.xfrm.x, l.xfrm.y))
        .collect();
    assert!(v.contains(&(0.0, 0.0)));
    assert!(v.contains(&(2_000_000.0, 0.0)));
    assert!(!v.contains(&(1_000_000.0, 0.0)));
}

#[test]
fn row_span_merges_down() {
    let t = tbl(
        "<a:tblPr/>",
        &[1_000_000, 1_000_000],
        &[
            tr(
                400_000,
                &[tc(r#"rowSpan="2""#, "tall", "", RED), plain("a")],
            ),
            tr(300_000, &[tc(r#"vMerge="1""#, "", "", ""), plain("b")]),
        ],
    );
    let s = scene(&frame(0, 0, 2_000_000, 700_000, &t));
    let r = rects(&s);
    assert_eq!((r[0].xfrm.w, r[0].xfrm.h), (1_000_000.0, 700_000.0));
    assert_eq!(r.len(), 3, "origin, a, b");
}

#[test]
fn a_row_grows_to_its_text() {
    let long = "word ".repeat(120);
    let t = tbl(
        "<a:tblPr/>",
        &[1_500_000],
        &[
            tr(300_000, &[plain(&long)]),
            tr(300_000, &[tc("", "x", "", RED)]),
        ],
    );
    let s = scene(&frame(0, 0, 1_500_000, 600_000, &t));
    let r = rects(&s);
    assert!(r[0].xfrm.h > 600_000.0, "grew: {}", r[0].xfrm.h);
    // The second row starts where the grown first row ends.
    assert_eq!(r[1].xfrm.y, r[0].xfrm.h);
    assert_eq!(r[1].xfrm.h, 300_000.0, "a row that fits keeps its height");
}

#[test]
fn an_empty_cell_is_as_high_as_one_line_and_margins() {
    let t = tbl(
        "<a:tblPr/>",
        &[1_000_000],
        &[tr(1_000, &[tc("", "", "", RED)])],
    );
    let s = scene(&frame(0, 0, 1_000_000, 1_000, &t));
    let r = rects(&s);
    // 10 pt line (12 pt high) + 2 x 45 720 EMU.
    let want = 12.0 * 12_700.0 + 2.0 * 45_720.0;
    assert!(
        (r[0].xfrm.h - want).abs() < 2_000.0,
        "{} vs {want}",
        r[0].xfrm.h
    );
}

#[test]
fn margins_shrink_the_text_width_and_count_in_the_growth() {
    let text = "alpha beta gamma delta epsilon zeta eta theta";
    let narrow = tc(
        "",
        text,
        r#"marL="400000" marR="400000" marT="200000" marB="200000""#,
        "",
    );
    let wide = tc("", text, "", "");
    let height = |cell: String| {
        let t = tbl("<a:tblPr/>", &[2_000_000], &[tr(1_000, &[cell])]);
        let s = scene(&frame(0, 0, 2_000_000, 1_000, &t));
        rects(&s)[0].xfrm.h
    };
    assert!(height(narrow) > height(wide) + 300_000.0);
}

#[test]
fn a_cell_spanning_rows_adds_what_is_missing_to_its_last_row() {
    let long = "word ".repeat(150);
    let t = tbl(
        "<a:tblPr/>",
        &[1_000_000, 1_000_000],
        &[
            tr(300_000, &[tc(r#"rowSpan="2""#, &long, "", RED), plain("a")]),
            tr(300_000, &[tc(r#"vMerge="1""#, "", "", ""), plain("b")]),
        ],
    );
    let s = scene(&frame(0, 0, 2_000_000, 600_000, &t));
    let r = rects(&s);
    // First row unchanged (a's row), second row took the extra.
    assert_eq!(r[1].xfrm.h, 300_000.0);
    assert!(r[2].xfrm.h > 300_000.0);
    assert_eq!(r[0].xfrm.h, r[1].xfrm.h + r[2].xfrm.h);
}

// ---------------------------------------------------------------------------------------------
// tcPr
// ---------------------------------------------------------------------------------------------

fn body_of(cell: String) -> sd::TextBody {
    let t = tbl("<a:tblPr/>", &[2_000_000], &[tr(500_000, &[cell])]);
    let s = scene(&frame(0, 0, 2_000_000, 500_000, &t));
    rects(&s)[0].text.clone().expect("text")
}

#[test]
fn tc_pr_margins_default_and_set() {
    assert_eq!(
        body_of(plain("a")).insets,
        (91_440.0, 45_720.0, 91_440.0, 45_720.0)
    );
    assert_eq!(
        body_of(tc("", "a", r#"marL="1" marR="2" marT="3" marB="4""#, "")).insets,
        (1.0, 3.0, 2.0, 4.0)
    );
    // A negative margin is no margin.
    assert_eq!(body_of(tc("", "a", r#"marL="-5""#, "")).insets.0, 0.0);
}

#[test]
fn tc_pr_anchor_anchor_ctr_and_vert() {
    assert_eq!(body_of(plain("a")).anchor, sd::Anchor::Top);
    assert_eq!(
        body_of(tc("", "a", r#"anchor="ctr""#, "")).anchor,
        sd::Anchor::Middle
    );
    assert_eq!(
        body_of(tc("", "a", r#"anchor="b""#, "")).anchor,
        sd::Anchor::Bottom
    );
    assert!(body_of(tc("", "a", r#"anchorCtr="1""#, "")).anchor_ctr);
    assert!(!body_of(plain("a")).anchor_ctr);
    for (v, want) in [
        ("vert", sd::Vert::Vert),
        ("vert270", sd::Vert::Vert270),
        ("eaVert", sd::Vert::EaVert),
        ("wordArtVert", sd::Vert::EaVert),
        ("mongolianVert", sd::Vert::Vert),
        ("horz", sd::Vert::Horz),
    ] {
        assert_eq!(
            body_of(tc("", "a", &format!(r#"vert="{v}""#), "")).vert,
            want,
            "{v}"
        );
    }
}

#[test]
fn a_cell_with_vertical_text_does_not_grow_its_row() {
    let long = "word ".repeat(100);
    let t = tbl(
        "<a:tblPr/>",
        &[500_000],
        &[tr(300_000, &[tc("", &long, r#"vert="vert""#, RED)])],
    );
    let s = scene(&frame(0, 0, 500_000, 300_000, &t));
    assert_eq!(rects(&s)[0].xfrm.h, 300_000.0);
}

#[test]
fn tc_pr_fill_kinds() {
    let grad = r#"<a:gradFill><a:gsLst><a:gs pos="0"><a:srgbClr val="FF0000"/></a:gs><a:gs pos="100000"><a:srgbClr val="0000FF"/></a:gs></a:gsLst><a:lin ang="0"/></a:gradFill>"#;
    let patt = r#"<a:pattFill prst="pct50"><a:fgClr><a:srgbClr val="FF0000"/></a:fgClr><a:bgClr><a:srgbClr val="FFFFFF"/></a:bgClr></a:pattFill>"#;
    let t = tbl(
        "<a:tblPr/>",
        &[1_000_000, 1_000_000, 1_000_000, 1_000_000],
        &[tr(
            400_000,
            &[
                tc("", "", "", RED),
                tc("", "", "", grad),
                tc("", "", "", patt),
                tc("", "t", "", "<a:noFill/>"),
            ],
        )],
    );
    let s = scene(&frame(0, 0, 4_000_000, 400_000, &t));
    let r = rects(&s);
    assert!(matches!(r[0].fill, Fill::Solid(_)));
    assert!(matches!(r[1].fill, Fill::Gradient(_)));
    assert!(matches!(r[2].fill, Fill::Pattern { .. }));
    assert!(!r[3].fill.is_visible());
}

#[test]
fn a_direct_no_fill_overrides_the_style() {
    let t = tbl(
        &pr(r#"firstRow="1""#, MS2_ACCENT1),
        &[1_000_000, 1_000_000],
        &[tr(
            400_000,
            &[plain("styled"), tc("", "plain", "", "<a:noFill/>")],
        )],
    );
    let s = scene(&frame(0, 0, 2_000_000, 400_000, &t));
    let r = rects(&s);
    assert_eq!(solid(&r[0].fill), Rgba::rgb(0x44, 0x72, 0xC4));
    assert!(!r[1].fill.is_visible());
}

#[test]
fn explicit_borders_and_diagonals() {
    let inner = ln("lnL", 12700, RED)
        + &ln("lnR", 25400, BLUE)
        + &ln("lnT", 12700, RED)
        + &ln("lnB", 12700, RED)
        + &ln("lnTlToBr", 12700, RED)
        + &ln("lnBlToTr", 12700, BLUE);
    let t = tbl(
        "<a:tblPr/>",
        &[1_000_000],
        &[tr(400_000, &[tc("", "a", "", &inner)])],
    );
    let s = scene(&frame(0, 0, 1_000_000, 400_000, &t));
    assert_eq!(hlines(&s).len(), 2);
    assert_eq!(vlines(&s).len(), 2);
    let right = vlines(&s)
        .into_iter()
        .find(|l| l.xfrm.x == 1_000_000.0)
        .unwrap();
    assert_eq!(right.line.as_ref().unwrap().width, 25_400.0);
    assert_eq!(line_color(right), Rgba::rgb(0, 0, 255));
    let diags: Vec<_> = lines(&s)
        .into_iter()
        .filter(|l| l.xfrm.w > 0.0 && l.xfrm.h > 0.0)
        .collect();
    assert_eq!(diags.len(), 2);
    assert!(!diags[0].xfrm.flip_v, "top-left to bottom-right");
    assert!(diags[1].xfrm.flip_v, "bottom-left to top-right");
}

#[test]
fn a_border_without_width_is_one_point_and_an_empty_one_is_unset() {
    let inner = r#"<a:lnL><a:solidFill><a:srgbClr val="FF0000"/></a:solidFill></a:lnL><a:lnR/>"#;
    let t = tbl(
        "<a:tblPr/>",
        &[1_000_000],
        &[tr(400_000, &[tc("", "a", "", inner)])],
    );
    let s = scene(&frame(0, 0, 1_000_000, 400_000, &t));
    let v = vlines(&s);
    assert_eq!(v.len(), 1);
    assert_eq!(v[0].line.as_ref().unwrap().width, 12_700.0);
}

#[test]
fn on_a_shared_edge_the_later_explicit_border_wins() {
    let a = tc("", "a", "", &ln("lnR", 12700, RED));
    let b = tc("", "b", "", &ln("lnL", 12700, BLUE));
    let t = tbl(
        "<a:tblPr/>",
        &[1_000_000, 1_000_000],
        &[tr(400_000, &[a, b])],
    );
    let s = scene(&frame(0, 0, 2_000_000, 400_000, &t));
    let mid: Vec<_> = vlines(&s)
        .into_iter()
        .filter(|l| l.xfrm.x == 1_000_000.0)
        .collect();
    assert_eq!(mid.len(), 1, "one line per edge");
    assert_eq!(line_color(mid[0]), Rgba::rgb(0, 0, 255));
    // With only the left cell's border set, that one is drawn.
    let a = tc("", "a", "", &ln("lnR", 12700, RED));
    let t = tbl(
        "<a:tblPr/>",
        &[1_000_000, 1_000_000],
        &[tr(400_000, &[a, plain("b")])],
    );
    let s = scene(&frame(0, 0, 2_000_000, 400_000, &t));
    assert_eq!(line_color(vlines(&s)[0]), Rgba::rgb(255, 0, 0));
}

#[test]
fn a_direct_no_line_removes_a_style_border() {
    // The style draws white inner lines; the direct `noFill` of the right cell's left border
    // (the later cell) removes the one between them.
    let b = tc("", "b", "", r#"<a:lnL><a:noFill/></a:lnL>"#);
    let t = tbl(
        &pr("", MS2_ACCENT1),
        &[1_000_000, 1_000_000],
        &[tr(400_000, &[plain("a"), b])],
    );
    let s = scene(&frame(0, 0, 2_000_000, 400_000, &t));
    let xs: Vec<f64> = vlines(&s).iter().map(|l| l.xfrm.x).collect();
    assert!(xs.contains(&0.0) && xs.contains(&2_000_000.0));
    assert!(!xs.contains(&1_000_000.0), "{xs:?}");
}

#[test]
fn equal_neighbouring_segments_are_one_line() {
    let b = ln("lnT", 12700, RED);
    let t = tbl(
        "<a:tblPr/>",
        &[1_000_000, 1_000_000, 1_000_000],
        &[tr(
            400_000,
            &[
                tc("", "a", "", &b),
                tc("", "b", "", &b),
                tc("", "c", "", &b),
            ],
        )],
    );
    let s = scene(&frame(0, 0, 3_000_000, 400_000, &t));
    let h = hlines(&s);
    assert_eq!(h.len(), 1);
    assert_eq!(h[0].xfrm.w, 3_000_000.0);
}

// ---------------------------------------------------------------------------------------------
// table styles
// ---------------------------------------------------------------------------------------------

fn styled(flags: &str, id: &str, rows: usize, cols: usize) -> SlideScene {
    let cells: Vec<String> = (0..cols).map(|c| plain(&format!("c{c}"))).collect();
    let rws: Vec<String> = (0..rows).map(|_| tr(400_000, &cells)).collect();
    let t = tbl(&pr(flags, id), &vec![1_000_000; cols], &rws);
    scene(&frame(
        0,
        0,
        1_000_000 * cols as i64,
        400_000 * rows as i64,
        &t,
    ))
}

fn lum(c: Rgba) -> u32 {
    u32::from(c.r) + u32::from(c.g) + u32::from(c.b)
}

#[test]
fn medium_style_2_is_built_in_and_applies_header_bands_and_text() {
    let s = styled(r#"firstRow="1" bandRow="1""#, MS2_ACCENT1, 4, 2);
    let r = rects(&s);
    let accent = Rgba::rgb(0x44, 0x72, 0xC4);
    // Header: accent fill, bold white text.
    assert_eq!(solid(&r[0].fill), accent);
    let run = &r[0].text.as_ref().unwrap().paragraphs[0].runs[0];
    assert!(run.bold);
    assert_eq!(run.fill, Fill::Solid(Rgba::WHITE));
    // First data row is band 1 (tint 40 %), the next one the whole-table tint 20 % (lighter).
    let band1 = solid(&r[2].fill);
    let band2 = solid(&r[4].fill);
    assert!(lum(band1) < lum(band2) && lum(band2) < lum(Rgba::WHITE));
    assert!(lum(band1) > lum(accent));
    assert_eq!(solid(&r[6].fill), band1, "bands alternate");
    // Body text: not bold, text colour.
    let run = &r[2].text.as_ref().unwrap().paragraphs[0].runs[0];
    assert!(!run.bold);
    assert_eq!(run.fill, Fill::Solid(Rgba::BLACK));
}

#[test]
fn the_headers_thick_bottom_border_beats_the_thin_inside_line() {
    let s = styled(r#"firstRow="1" bandRow="1""#, MS2_ACCENT1, 3, 2);
    let h = hlines(&s);
    let widths: Vec<(f64, f64)> = h
        .iter()
        .map(|l| (l.xfrm.y, l.line.as_ref().unwrap().width))
        .collect();
    // Edge below the header (y = 400 000) is 3 pt, the others 1 pt.
    assert!(widths.contains(&(400_000.0, 38_100.0)), "{widths:?}");
    assert!(widths.contains(&(800_000.0, 12_700.0)), "{widths:?}");
    assert!(widths.contains(&(0.0, 12_700.0)));
    // (Neighbouring equal segments are one line per edge.)
    assert_eq!(h.len(), 4);
    assert!(h.iter().all(|l| line_color(l) == Rgba::WHITE));
}

#[test]
fn last_row_first_and_last_column_and_column_bands() {
    let s = styled(
        r#"lastRow="1" firstCol="1" lastCol="1" bandCol="1""#,
        MS2_ACCENT1,
        3,
        4,
    );
    let r = rects(&s);
    let accent = Rgba::rgb(0x44, 0x72, 0xC4);
    let at = |row: usize, col: usize| solid(&r[row * 4 + col].fill);
    assert_eq!(at(0, 0), accent, "first column");
    assert_eq!(at(0, 3), accent, "last column");
    assert_eq!(at(2, 1), accent, "last row");
    // Band columns count from the column after the first one: column 1 is band 1.
    assert_eq!(lum(at(0, 1)), lum(at(1, 1)));
    assert!(lum(at(0, 1)) < lum(at(0, 2)), "band1V is the darker tint");
    let run = &r[0].text.as_ref().unwrap().paragraphs[0].runs[0];
    assert!(run.bold);
    // The last row's top border is 3 pt.
    assert!(hlines(&s)
        .iter()
        .any(|l| l.xfrm.y == 800_000.0 && l.line.as_ref().unwrap().width == 38_100.0));
}

#[test]
fn without_flags_only_the_whole_table_part_applies() {
    let s = styled("", MS2_ACCENT1, 3, 2);
    let r = rects(&s);
    let first = solid(&r[0].fill);
    assert!(r.iter().all(|c| solid(&c.fill) == first));
    assert!(!r[0].text.as_ref().unwrap().paragraphs[0].runs[0].bold);
}

#[test]
fn the_other_built_in_ids_use_their_own_colour_and_unknown_ids_no_style() {
    let accent2 = styled(
        r#"firstRow="1""#,
        "{21E4AEA4-8DFA-4A89-87EB-49C32662AFE0}",
        2,
        1,
    );
    assert_eq!(solid(&rects(&accent2)[0].fill), Rgba::rgb(0xED, 0x7D, 0x31));
    let dark = styled(
        r#"firstRow="1""#,
        "{073A0DAA-6AF3-43AB-8588-CEC1D06C72B9}",
        2,
        1,
    );
    assert_eq!(solid(&rects(&dark)[0].fill), Rgba::BLACK);
    let unknown = styled(
        r#"firstRow="1" bandRow="1""#,
        "{11111111-2222-3333-4444-555555555555}",
        3,
        2,
    );
    assert!(rects(&unknown).iter().all(|c| !c.fill.is_visible()));
    assert!(lines(&unknown).is_empty());
    let none = styled(r#"firstRow="1""#, "", 2, 2);
    assert!(rects(&none).iter().all(|c| !c.fill.is_visible()));
}

const CUSTOM: &str = r#"<a:tblStyle styleId="{AAAAAAAA-0000-0000-0000-000000000001}" styleName="Mine"><a:wholeTbl><a:tcTxStyle i="on"><a:fontRef idx="major"/><a:srgbClr val="00FF00"/></a:tcTxStyle><a:tcStyle><a:tcBdr><a:left><a:ln w="9525"><a:solidFill><a:srgbClr val="FF0000"/></a:solidFill></a:ln></a:left><a:insideV><a:ln w="19050"><a:solidFill><a:srgbClr val="0000FF"/></a:solidFill></a:ln></a:insideV></a:tcBdr><a:fill><a:solidFill><a:srgbClr val="EEEEEE"/></a:solidFill></a:fill></a:tcStyle></a:wholeTbl><a:firstRow><a:tcTxStyle b="on"><a:fontRef idx="minor"/></a:tcTxStyle><a:tcStyle><a:tcBdr/><a:fill><a:solidFill><a:srgbClr val="112233"/></a:solidFill></a:fill></a:tcStyle></a:firstRow><a:neCell><a:tcStyle><a:tcBdr/><a:fill><a:solidFill><a:srgbClr val="445566"/></a:solidFill></a:fill></a:tcStyle></a:neCell></a:tblStyle>"#;

fn with_custom(flags: &str, id: &str, rows: usize, cols: usize) -> SlideScene {
    let cells: Vec<String> = (0..cols).map(|c| plain(&format!("c{c}"))).collect();
    let rws: Vec<String> = (0..rows).map(|_| tr(400_000, &cells)).collect();
    let t = tbl(&pr(flags, id), &vec![1_000_000; cols], &rws);
    scene_with(
        &frame(0, 0, 1_000_000 * cols as i64, 400_000 * rows as i64, &t),
        Some(CUSTOM),
        &DocOptions::default(),
    )
}

#[test]
fn a_stored_style_is_read_and_its_parts_override_property_by_property() {
    let s = with_custom(
        r#"firstRow="1" lastCol="1""#,
        "{AAAAAAAA-0000-0000-0000-000000000001}",
        2,
        3,
    );
    let r = rects(&s);
    assert_eq!(
        solid(&r[0].fill),
        Rgba::rgb(0x11, 0x22, 0x33),
        "firstRow over wholeTbl"
    );
    assert_eq!(solid(&r[3].fill), Rgba::rgb(0xEE, 0xEE, 0xEE));
    let head = &r[0].text.as_ref().unwrap().paragraphs[0].runs[0];
    assert!(head.bold, "firstRow sets bold");
    assert!(head.italic, "wholeTbl's italic stays");
    assert_eq!(
        head.fill,
        Fill::Solid(Rgba::rgb(0, 0xFF, 0)),
        "colour from wholeTbl"
    );
    let body = &r[3].text.as_ref().unwrap().paragraphs[0].runs[0];
    assert!(!body.bold && body.italic);
}

#[test]
fn style_borders_outer_and_inside_and_edge_of_the_part_region() {
    let s = with_custom("", "{AAAAAAAA-0000-0000-0000-000000000001}", 1, 3);
    let v: Vec<(f64, f64, Rgba)> = vlines(&s)
        .into_iter()
        .map(|l| (l.xfrm.x, l.line.as_ref().unwrap().width, line_color(l)))
        .collect();
    // Left edge: `left`; the edges between columns: `insideV`; the right edge: not set.
    assert_eq!(
        v,
        vec![
            (0.0, 9_525.0, Rgba::rgb(255, 0, 0)),
            (1_000_000.0, 19_050.0, Rgba::rgb(0, 0, 255)),
            (2_000_000.0, 19_050.0, Rgba::rgb(0, 0, 255)),
        ]
    );
}

#[test]
fn a_corner_part_applies_when_both_flags_are_on() {
    let on = with_custom(
        r#"firstRow="1" lastCol="1""#,
        "{AAAAAAAA-0000-0000-0000-000000000001}",
        2,
        2,
    );
    assert_eq!(solid(&rects(&on)[1].fill), Rgba::rgb(0x44, 0x55, 0x66));
    let off = with_custom(
        r#"firstRow="1""#,
        "{AAAAAAAA-0000-0000-0000-000000000001}",
        2,
        2,
    );
    assert_eq!(solid(&rects(&off)[1].fill), Rgba::rgb(0x11, 0x22, 0x33));
}

#[test]
fn a_stored_definition_wins_over_the_built_in_of_the_same_id() {
    let own = format!(
        r#"<a:tblStyle styleId="{MS2_ACCENT1}"><a:wholeTbl><a:tcStyle><a:tcBdr/><a:fill><a:solidFill><a:srgbClr val="010203"/></a:solidFill></a:fill></a:tcStyle></a:wholeTbl></a:tblStyle>"#
    );
    let t = tbl(
        &pr("", MS2_ACCENT1),
        &[1_000_000],
        &[tr(400_000, &[plain("a")])],
    );
    let s = scene_with(
        &frame(0, 0, 1_000_000, 400_000, &t),
        Some(&own),
        &DocOptions::default(),
    );
    assert_eq!(solid(&rects(&s)[0].fill), Rgba::rgb(1, 2, 3));
}

#[test]
fn an_inline_style_and_a_table_background_and_theme_references() {
    let inline = r#"<a:tblStyle styleId="x"><a:tblBg><a:solidFill><a:srgbClr val="ABCDEF"/></a:solidFill></a:tblBg><a:wholeTbl><a:tcStyle><a:tcBdr><a:top><a:lnRef idx="2"><a:schemeClr val="accent2"/></a:lnRef></a:top></a:tcBdr><a:fillRef idx="1"><a:schemeClr val="accent6"/></a:fillRef></a:tcStyle></a:wholeTbl></a:tblStyle>"#;
    let t = tbl(
        &format!("<a:tblPr>{inline}</a:tblPr>"),
        &[1_000_000],
        &[tr(400_000, &[plain("a")])],
    );
    let s = scene(&frame(0, 0, 1_000_000, 400_000, &t));
    // `fillRef idx=1` (the theme's solid phClr) with accent6 beats the table background.
    assert_eq!(solid(&rects(&s)[0].fill), Rgba::rgb(0x70, 0xAD, 0x47));
    let top = hlines(&s).into_iter().find(|l| l.xfrm.y == 0.0).unwrap();
    assert_eq!(top.line.as_ref().unwrap().width, 12_700.0, "theme line 2");
    assert_eq!(line_color(top), Rgba::rgb(0xED, 0x7D, 0x31));
    // The background alone:
    let bg_only = r#"<a:tblStyle styleId="x"><a:tblBg><a:solidFill><a:srgbClr val="ABCDEF"/></a:solidFill></a:tblBg></a:tblStyle>"#;
    let t = tbl(
        &format!("<a:tblPr>{bg_only}</a:tblPr>"),
        &[1_000_000],
        &[tr(400_000, &[plain("a")])],
    );
    let s = scene(&frame(0, 0, 1_000_000, 400_000, &t));
    assert_eq!(solid(&rects(&s)[0].fill), Rgba::rgb(0xAB, 0xCD, 0xEF));
}

#[test]
fn rtl_mirrors_the_columns() {
    let t = tbl(
        &pr(r#"rtl="1""#, ""),
        &[1_000_000, 3_000_000],
        &[tr(
            400_000,
            &[tc("", "first", "", RED), tc("", "second", "", BLUE)],
        )],
    );
    let s = scene(&frame(100_000, 0, 4_000_000, 400_000, &t));
    let r = rects(&s);
    assert_eq!(
        r[0].xfrm.x,
        100_000.0 + 3_000_000.0,
        "first column on the right"
    );
    assert_eq!(r[1].xfrm.x, 100_000.0);
    assert_eq!(r[1].xfrm.w, 3_000_000.0);
}

#[test]
fn text_inherits_the_default_style_and_a_run_beats_the_table_style() {
    let own = r#"<a:p><a:r><a:rPr lang="en-US" sz="1000" b="0"><a:solidFill><a:srgbClr val="123456"/></a:solidFill></a:rPr><a:t>run</a:t></a:r></a:p>"#;
    let cell =
        format!(r#"<a:tc><a:txBody><a:bodyPr/><a:lstStyle/>{own}</a:txBody><a:tcPr/></a:tc>"#);
    let t = tbl(
        &pr(r#"firstRow="1""#, MS2_ACCENT1),
        &[1_000_000],
        &[tr(400_000, &[cell])],
    );
    let s = scene(&frame(0, 0, 1_000_000, 400_000, &t));
    let run = &rects(&s)[0].text.as_ref().unwrap().paragraphs[0].runs[0];
    assert!(!run.bold, "the run's b=0 beats the header style");
    assert_eq!(run.fill, Fill::Solid(Rgba::rgb(0x12, 0x34, 0x56)));
}

// ---------------------------------------------------------------------------------------------
// budgets and hostile tables
// ---------------------------------------------------------------------------------------------

#[test]
fn no_columns_or_no_rows_draw_nothing() {
    let t = tbl("<a:tblPr/>", &[], &[tr(400_000, &[plain("a")])]);
    let s = scene(&frame(0, 0, 1_000_000, 400_000, &t));
    assert!(s.items.is_empty());
    let t = tbl("<a:tblPr/>", &[1_000_000], &[]);
    let s = scene(&frame(0, 0, 1_000_000, 400_000, &t));
    assert!(s.items.is_empty());
}

#[test]
fn spans_are_clamped_to_the_grid() {
    let t = tbl(
        "<a:tblPr/>",
        &[1_000_000, 1_000_000],
        &[
            tr(
                400_000,
                &[
                    tc(
                        r#"gridSpan="1000000000" rowSpan="999999999""#,
                        "big",
                        "",
                        RED,
                    ),
                    plain("b"),
                ],
            ),
            tr(400_000, &[plain("c"), plain("d")]),
        ],
    );
    let s = scene(&frame(0, 0, 2_000_000, 800_000, &t));
    let r = rects(&s);
    assert_eq!((r[0].xfrm.w, r[0].xfrm.h), (2_000_000.0, 800_000.0));
    // (`b`, `c` and `d` are covered by the origin cell; they draw nothing.)
    assert_eq!(r.len(), 1);
}

#[test]
fn a_span_does_not_take_cells_an_earlier_cell_owns() {
    // Row 1's first cell spans down over (1, 0); the second row's cell at column 0 is the orphan
    // owner's, and the cell at column 1 asks for 2 columns: clamped to the grid.
    let t = tbl(
        "<a:tblPr/>",
        &[1_000_000, 1_000_000],
        &[
            tr(400_000, &[tc(r#"rowSpan="2""#, "a", "", RED), plain("b")]),
            tr(
                400_000,
                &[plain("hidden"), tc(r#"gridSpan="2""#, "c", "", BLUE)],
            ),
        ],
    );
    let s = scene(&frame(0, 0, 2_000_000, 800_000, &t));
    let r = rects(&s);
    assert_eq!(r.len(), 3);
    assert_eq!(r[2].xfrm.w, 1_000_000.0);
}

#[test]
fn an_orphan_merge_cell_is_drawn_without_its_text() {
    let t = tbl(
        &pr(r#"firstRow="1""#, MS2_ACCENT1),
        &[1_000_000],
        &[tr(400_000, &[tc(r#"hMerge="1""#, "ghost", "", "")])],
    );
    let s = scene(&frame(0, 0, 1_000_000, 400_000, &t));
    let r = rects(&s);
    assert_eq!(r.len(), 1);
    assert!(r[0].text.is_none());
    assert!(r[0].fill.is_visible(), "its style fill still draws");
}

#[test]
fn negative_widths_and_heights_are_zero() {
    let t = tbl(
        "<a:tblPr/>",
        &[-500, 1_000_000],
        &[tr(-100, &[tc("", "", "", RED), tc("", "", "", BLUE)])],
    );
    let s = scene(&frame(0, 0, 1_000_000, 400_000, &t));
    let r = rects(&s);
    assert_eq!(r[0].xfrm.w, 0.0);
    assert!(r[1].xfrm.x == 0.0 && r[1].xfrm.w == 1_000_000.0);
    assert!(r[0].xfrm.h > 0.0, "grew to the empty line");
}

#[test]
fn extra_cells_in_a_row_are_dropped_and_flagged() {
    let t = tbl(
        "<a:tblPr/>",
        &[1_000_000],
        &[tr(400_000, &[tc("", "", "", RED), tc("", "", "", BLUE)])],
    );
    let s = scene(&frame(0, 0, 1_000_000, 400_000, &t));
    assert_eq!(rects(&s).len(), 1);
    assert!(s.truncated);
}

#[test]
fn too_many_rows_columns_and_cells_are_cut_and_flagged() {
    // Rows.
    let cells = [plain("a")];
    let rows: Vec<String> = (0..MAX_TABLE_ROWS + 5).map(|_| tr(1_000, &cells)).collect();
    let t = tbl("<a:tblPr/>", &[100_000], &rows);
    let s = scene(&frame(0, 0, 100_000, 1_000, &t));
    assert!(s.truncated);
    assert_eq!(rects(&s).len(), MAX_TABLE_ROWS);
    // Columns.
    let n = MAX_TABLE_COLS + 3;
    let cells: Vec<String> = (0..n).map(|_| tc("", "", "", RED)).collect();
    let t = tbl("<a:tblPr/>", &vec![10_000; n], &[tr(1_000, &cells)]);
    let s = scene(&frame(0, 0, 10_000 * n as i64, 1_000, &t));
    assert!(s.truncated);
    assert_eq!(rects(&s).len(), MAX_TABLE_COLS);
    // Cells: rows x columns is capped, rows are dropped.
    let n = 80;
    let cells: Vec<String> = (0..n).map(|_| tc("", "", "", RED)).collect();
    let rows: Vec<String> = (0..200).map(|_| tr(1_000, &cells)).collect();
    let t = tbl("<a:tblPr/>", &vec![10_000; n], &rows);
    let s = scene(&frame(0, 0, 10_000 * n as i64, 1_000, &t));
    assert!(s.truncated);
    assert_eq!(rects(&s).len(), MAX_TABLE_CELLS / n * n);
    assert!(rects(&s).len() <= MAX_TABLE_CELLS);
}

#[test]
fn ten_thousand_rows_do_not_blow_up() {
    let cells = [plain("a"), plain("b")];
    let rows: Vec<String> = (0..10_000).map(|_| tr(1_000, &cells)).collect();
    let t = tbl("<a:tblPr/>", &[100_000, 100_000], &rows);
    let s = scene(&frame(0, 0, 200_000, 1_000, &t));
    assert!(s.truncated);
    assert!(rects(&s).len() <= MAX_TABLE_CELLS);
}

#[test]
fn the_slides_item_budget_stops_a_table_and_says_so() {
    let cells = [tc("", "a", "", RED), tc("", "b", "", RED)];
    let rows: Vec<String> = (0..10).map(|_| tr(100_000, &cells)).collect();
    let t = tbl("<a:tblPr/>", &[100_000, 100_000], &rows);
    let opts = DocOptions {
        max_slide_shapes: 5,
        ..DocOptions::default()
    };
    let s = scene_with(&frame(0, 0, 200_000, 1_000_000, &t), None, &opts);
    assert!(s.truncated);
    assert!(s.items.len() <= 5);
}

#[test]
fn a_cells_text_is_capped() {
    let long = "x".repeat(MAX_CELL_CHARS + 500);
    let t = tbl("<a:tblPr/>", &[1_000_000], &[tr(400_000, &[plain(&long)])]);
    let s = scene(&frame(0, 0, 1_000_000, 400_000, &t));
    assert!(s.truncated);
    let n: usize = rects(&s)[0]
        .text
        .as_ref()
        .unwrap()
        .paragraphs
        .iter()
        .flat_map(|p| &p.runs)
        .map(|r| r.text.chars().count())
        .sum();
    assert_eq!(n, MAX_CELL_CHARS);
}

#[test]
fn cap_cell_text_cuts_across_runs_and_paragraphs() {
    let run = |n: usize| sd::Run::text("y".repeat(n), 10.0);
    let para = |runs: Vec<sd::Run>| sd::Paragraph {
        runs,
        ..sd::Paragraph::default()
    };
    let mut body = sd::TextBody {
        paragraphs: vec![
            para(vec![run(MAX_CELL_CHARS - 3), run(10)]),
            para(vec![run(5)]),
        ],
        ..sd::TextBody::default()
    };
    assert!(cap_cell_text(&mut body));
    assert_eq!(body.paragraphs.len(), 1);
    assert_eq!(body.paragraphs[0].runs[1].text.chars().count(), 3);
    let mut small = sd::TextBody {
        paragraphs: vec![para(vec![run(4)])],
        ..sd::TextBody::default()
    };
    assert!(!cap_cell_text(&mut small));
}

#[test]
fn more_styles_than_the_budget_are_cut_and_flagged() {
    let mut xml = String::from(
        r#"<a:tblStyleLst xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main">"#,
    );
    for i in 0..MAX_TABLE_STYLES + 3 {
        xml += &format!(r#"<a:tblStyle styleId="{{S{i}}}"/>"#);
    }
    xml += "</a:tblStyleLst>";
    let ts = TableStyles::parse(xml.as_bytes(), &DocOptions::default());
    assert_eq!(ts.styles.len(), MAX_TABLE_STYLES);
    assert!(ts.truncated);
    assert!(ts.get("{S0}").is_some());
    assert!(ts.get("{S200}").is_none());
    // Garbage keeps what was read.
    let ts = TableStyles::parse(
        b"<a:tblStyleLst><a:tblStyle styleId=\"k\"/><a:tbl",
        &DocOptions::default(),
    );
    assert!(ts.get("k").is_some());
    assert!(TableStyles::parse(b"", &DocOptions::default())
        .styles
        .is_empty());
}

#[test]
fn a_style_id_the_truncated_part_may_have_held_flags_the_scene() {
    let mut xml = String::new();
    for i in 0..MAX_TABLE_STYLES + 1 {
        xml += &format!(r#"<a:tblStyle styleId="{{S{i}}}"/>"#);
    }
    let t = tbl(
        &pr("", "{S999}"),
        &[1_000_000],
        &[tr(400_000, &[plain("a")])],
    );
    let s = scene_with(
        &frame(0, 0, 1_000_000, 400_000, &t),
        Some(&xml),
        &DocOptions::default(),
    );
    assert!(s.truncated);
}

#[test]
fn the_part_order_and_precedence() {
    let f = Flags {
        first_row: true,
        last_row: true,
        first_col: true,
        last_col: true,
        band_row: true,
        band_col: true,
    };
    // A cell that is the top-left corner: no bands, both first parts and the corner.
    assert_eq!(
        f.parts(0, 0, 4, 4),
        vec![Part::Whole, Part::FirstCol, Part::FirstRow, Part::NwCell]
    );
    // An inner cell: both bands (bands count after the header row / first column).
    assert_eq!(
        f.parts(1, 1, 4, 4),
        vec![Part::Whole, Part::Band1V, Part::Band1H]
    );
    assert_eq!(
        f.parts(2, 2, 4, 4),
        vec![Part::Whole, Part::Band2V, Part::Band2H]
    );
    assert_eq!(
        f.parts(3, 3, 4, 4),
        vec![Part::Whole, Part::LastCol, Part::LastRow, Part::SeCell]
    );
    assert!(Part::Whole.rank() < Part::Band1V.rank());
    assert!(Part::Band2H.rank() < Part::FirstCol.rank());
    assert!(Part::LastCol.rank() < Part::FirstRow.rank());
    assert!(Part::LastRow.rank() < Part::NeCell.rank());
    // A one-cell table is every first / last part at once.
    assert_eq!(f.parts(0, 0, 1, 1).len(), 1 + 2 + 2 + 4);
}

#[test]
fn resolve_prefers_rank_then_the_later_cell() {
    let line = |w: f64| Some(sd::Line::solid(w, Rgba::BLACK));
    let a: Cand = (4, line(3.0));
    let b: Cand = (0, line(1.0));
    assert_eq!(resolve(Some(&a), Some(&b)), line(3.0));
    assert_eq!(resolve(Some(&b), Some(&a)), line(3.0));
    let c: Cand = (0, line(2.0));
    assert_eq!(resolve(Some(&b), Some(&c)), line(2.0), "tie: the later");
    let none: Cand = (9, None);
    assert_eq!(resolve(Some(&a), Some(&none)), None, "no line wins by rank");
    assert_eq!(resolve(None, Some(&a)), line(3.0));
    assert_eq!(resolve(Some(&a), None), line(3.0));
    assert_eq!(resolve(None, None), None);
}

#[test]
fn tx_props_make_a_list_style_only_when_something_is_set() {
    assert!(TxProps::default().list_style().is_none());
    let p = TxProps {
        bold: Some(false),
        italic: Some(true),
        color: None,
        font: Some("+mj-lt"),
    };
    let n = p.list_style().unwrap();
    let d = n.child("defRPr").unwrap();
    assert_eq!(d.attr("b"), Some("0"));
    assert_eq!(d.attr("i"), Some("1"));
    assert_eq!(d.child("latin").unwrap().attr("typeface"), Some("+mj-lt"));
}

#[test]
fn builtin_ids_match_case_insensitively_and_unknown_have_none() {
    assert!(builtin_style("{5c22544a-7ee6-4342-b048-85bdc9fd1c3a}").is_some());
    assert!(builtin_style("{0}").is_none());
    let n = builtin_style(MS2_ACCENT1).unwrap();
    assert!(n.child("firstRow").is_some() && n.child("band1H").is_some());
}

#[test]
fn a_table_in_a_frame_without_a_table_or_with_hidden_frame_is_nothing() {
    let s = scene(
        r#"<p:graphicFrame><p:nvGraphicFramePr><p:cNvPr id="4" name="T" hidden="1"/><p:cNvGraphicFramePr/><p:nvPr/></p:nvGraphicFramePr><p:xfrm><a:off x="0" y="0"/><a:ext cx="1" cy="1"/></p:xfrm><a:graphic><a:graphicData uri="x"/></a:graphic></p:graphicFrame>"#,
    );
    assert!(s.items.is_empty());
}
