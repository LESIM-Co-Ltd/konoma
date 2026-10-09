//! Tests of the table drawing (`table.rs`): hand-written ODF tables in the form LibreOffice writes
//! them (the frame is at 2 cm, 1 cm; 6 x 3 cm).

use super::*;

/// Column and row styles, and the cell styles `c1` (a red fill, a 1 pt black border and 10 pt
/// text), `c2` (a blue fill, no border) and `c3` (no fill, a 2 pt green border on the left only).
const TABLE_STYLES: &str = r##"
<style:style style:name="co1" style:family="table-column"><style:table-column-properties style:column-width="2cm"/></style:style>
<style:style style:name="co2" style:family="table-column"><style:table-column-properties style:column-width="0cm" style:use-optimal-column-width="true"/></style:style>
<style:style style:name="ro1" style:family="table-row"><style:table-row-properties style:row-height="1cm"/></style:style>
<style:style style:name="ro2" style:family="table-row"><style:table-row-properties style:min-row-height="2cm"/></style:style>
<style:style style:name="c1" style:family="table-cell"><loext:graphic-properties draw:fill="solid" draw:fill-color="#ff0000"/><style:paragraph-properties fo:border="1pt solid #000000"/><style:text-properties fo:font-size="10pt"/></style:style>
<style:style style:name="c2" style:family="table-cell"><loext:graphic-properties draw:fill="solid" draw:fill-color="#0000ff"/><style:text-properties fo:font-size="10pt"/></style:style>
<style:style style:name="c3" style:family="table-cell"><style:table-cell-properties fo:border-left="2pt solid #00ff00" fo:border-right="none" fo:border-top="none" fo:border-bottom="none"/></style:style>"##;

fn table_op_with(attrs: &str, inner: &str, extra_styles: &str) -> Op {
    op(&slide_page(&pic(&format!(
        r#"<table:table table:name="T" {attrs}>{inner}</table:table>"#
    ))))
    .auto(&(gr("") + TABLE_STYLES + extra_styles))
}

fn table_op(inner: &str) -> Op {
    table_op_with("", inner, "")
}

fn cols(n: usize) -> String {
    r#"<table:table-column table:style-name="co1"/>"#.repeat(n)
}

fn cell(style: &str, text: &str) -> String {
    let st = if style.is_empty() {
        String::new()
    } else {
        format!(r#" table:style-name="{style}""#)
    };
    format!(r#"<table:table-cell{st}><text:p>{text}</text:p></table:table-cell>"#)
}

fn row(cells: &str) -> String {
    format!(r#"<table:table-row table:style-name="ro1">{cells}</table:table-row>"#)
}

fn rows(n: usize, cells: usize, style: &str) -> String {
    (0..n)
        .map(|r| {
            row(&(0..cells)
                .map(|c| cell(style, &format!("r{r}c{c}")))
                .collect::<String>())
        })
        .collect()
}

/// The shapes that are cells (rectangles with a fill or text), in order.
fn cells_of(sc: &sd::SlideScene) -> Vec<ShapeItem> {
    shapes_of(sc)
        .into_iter()
        .filter(|s| s.geom == Geometry::Rect)
        .cloned()
        .collect()
}

/// The border lines.
fn lines_of(sc: &sd::SlideScene) -> Vec<ShapeItem> {
    shapes_of(sc)
        .into_iter()
        .filter(|s| s.geom == Geometry::Line)
        .cloned()
        .collect()
}

fn cell_named(sc: &sd::SlideScene, text: &str) -> ShapeItem {
    cells_of(sc)
        .into_iter()
        .find(|s| text_of_shape(s) == text)
        .unwrap_or_else(|| panic!("no cell {text}"))
}

fn rgb(s: &ShapeItem) -> Option<Rgba> {
    match s.fill {
        Fill::Solid(c) => Some(c),
        _ => None,
    }
}

fn at(s: &ShapeItem) -> (f64, f64, f64, f64) {
    (
        s.xfrm.x / EMU_CM,
        s.xfrm.y / EMU_CM,
        s.xfrm.w / EMU_CM,
        s.xfrm.h / EMU_CM,
    )
}

fn near(a: (f64, f64, f64, f64), b: (f64, f64, f64, f64)) -> bool {
    (a.0 - b.0).abs() < 0.01
        && (a.1 - b.1).abs() < 0.01
        && (a.2 - b.2).abs() < 0.01
        && (a.3 - b.3).abs() < 0.01
}

#[test]
fn columns_and_rows_place_the_cells_from_the_frames_corner() {
    let sc = scene(&table_op(&(cols(3) + &rows(2, 3, "c1"))));
    assert_eq!(cells_of(&sc).len(), 6);
    assert!(near(at(&cell_named(&sc, "r0c0")), (2.0, 1.0, 2.0, 1.0)));
    assert!(near(at(&cell_named(&sc, "r0c2")), (6.0, 1.0, 2.0, 1.0)));
    assert!(near(at(&cell_named(&sc, "r1c1")), (4.0, 2.0, 2.0, 1.0)));
    assert_eq!(rgb(&cell_named(&sc, "r1c1")), Some(Rgba::rgb(255, 0, 0)));
}

#[test]
fn repeated_columns_and_rows_expand() {
    let inner = r#"<table:table-column table:style-name="co1" table:number-columns-repeated="3"/>
<table:table-row table:style-name="ro1" table:number-rows-repeated="2"><table:table-cell table:style-name="c1" table:number-columns-repeated="3"><text:p>x</text:p></table:table-cell></table:table-row>"#;
    let sc = scene(&table_op(inner));
    let cs = cells_of(&sc);
    assert_eq!(cs.len(), 6);
    assert!(near(at(&cs[5]), (6.0, 2.0, 2.0, 1.0)));
}

#[test]
fn header_and_group_containers_are_looked_through() {
    let inner = format!(
        r#"<table:table-header-columns><table:table-column table:style-name="co1"/></table:table-header-columns><table:table-columns><table:table-column table:style-name="co1"/></table:table-columns><table:table-header-rows>{}</table:table-header-rows><table:table-rows>{}</table:table-rows>"#,
        row(&(cell("c1", "h0") + &cell("c1", "h1"))),
        row(&(cell("c2", "b0") + &cell("c2", "b1")))
    );
    let sc = scene(&table_op(&inner));
    assert!(near(at(&cell_named(&sc, "h1")), (4.0, 1.0, 2.0, 1.0)));
    assert!(near(at(&cell_named(&sc, "b0")), (2.0, 2.0, 2.0, 1.0)));
}

#[test]
fn the_row_height_is_the_minimum_and_a_row_grows_to_its_text() {
    let long = "word ".repeat(60);
    let inner = format!(
        "{}{}{}",
        cols(1),
        row(&cell("c1", &long)),
        r#"<table:table-row table:style-name="ro2"><table:table-cell table:style-name="c2"><text:p>min</text:p></table:table-cell></table:table-row>"#
    );
    let sc = scene(&table_op(&inner));
    let cs = cells_of(&sc);
    let first = at(&cs[0]);
    assert!(first.3 > 3.0, "grew to the text: {first:?}");
    // the second row starts where the first ends and keeps its 2 cm minimum
    let second = at(&cs[1]);
    assert!((second.1 - (first.1 + first.3)).abs() < 0.01);
    assert!((second.3 - 2.0).abs() < 0.01);
}

#[test]
fn a_spanning_cell_covers_its_slots_and_covered_cells_are_never_drawn() {
    let span = r#"<table:table-cell table:style-name="c2" table:number-columns-spanned="2" table:number-rows-spanned="2"><text:p>M</text:p></table:table-cell>"#;
    let inner = format!(
        "{}{}{}{}",
        cols(3),
        row(&(span.to_string()
            + r#"<table:covered-table-cell table:style-name="c1"/>"#
            + &cell("c1", "a"))),
        row(
            &(r#"<table:covered-table-cell/><table:covered-table-cell/>"#.to_string()
                + &cell("c1", "b"))
        ),
        row(&(cell("c1", "x") + &cell("c1", "y") + &cell("c1", "z"))),
    );
    let sc = scene(&table_op(&inner));
    assert!(near(at(&cell_named(&sc, "M")), (2.0, 1.0, 4.0, 2.0)));
    assert!(near(at(&cell_named(&sc, "a")), (6.0, 1.0, 2.0, 1.0)));
    assert!(near(at(&cell_named(&sc, "b")), (6.0, 2.0, 2.0, 1.0)));
    // the covered cell's red fill is not drawn: the only blue is the merged cell
    assert_eq!(
        cells_of(&sc)
            .iter()
            .filter(|s| rgb(s) == Some(Rgba::rgb(255, 0, 0)))
            .count(),
        5
    );
    // no border line inside the merged cell
    assert!(lines_of(&sc).iter().all(|l| !(l.xfrm.x > 2.0 * EMU_CM + 1.0
        && l.xfrm.x < 6.0 * EMU_CM - 1.0
        && l.xfrm.y > 1.0 * EMU_CM + 1.0
        && l.xfrm.y < 3.0 * EMU_CM - 1.0
        && l.xfrm.y + l.xfrm.h < 3.0 * EMU_CM)));
}

#[test]
fn a_cell_after_a_span_without_covered_cells_moves_to_the_next_free_slot() {
    let span = r#"<table:table-cell table:style-name="c2" table:number-columns-spanned="2"><text:p>M</text:p></table:table-cell>"#;
    let inner = format!("{}{}", cols(3), row(&(span.to_string() + &cell("c1", "a"))));
    let sc = scene(&table_op(&inner));
    assert!(near(at(&cell_named(&sc, "M")), (2.0, 1.0, 4.0, 1.0)));
    assert!(near(at(&cell_named(&sc, "a")), (6.0, 1.0, 2.0, 1.0)));
}

#[test]
fn spans_past_the_grid_or_over_taken_slots_are_clamped() {
    let big = r#"<table:table-cell table:style-name="c2" table:number-columns-spanned="99" table:number-rows-spanned="99"><text:p>M</text:p></table:table-cell>"#;
    let inner = format!(
        "{}{}{}",
        cols(2),
        row(&(cell("c1", "a") + big)),
        row(&(cell("c1", "b") + &cell("c1", "c")))
    );
    let sc = scene(&table_op(&inner));
    // clamped to the grid: one column, and the second row's slot below is free
    assert!(near(at(&cell_named(&sc, "M")), (4.0, 1.0, 2.0, 2.0)));
    // the cell below the span is covered by it, so "c" has to move on and is dropped
    assert!(cells_of(&sc).iter().all(|s| text_of_shape(s) != "c"));
    assert!(near(at(&cell_named(&sc, "b")), (2.0, 2.0, 2.0, 1.0)));
}

#[test]
fn a_cell_style_fill_border_and_text_size_are_read() {
    let sc = scene(&table_op(&(cols(1) + &rows(1, 1, "c1"))));
    let c = cell_named(&sc, "r0c0");
    assert_eq!(rgb(&c), Some(Rgba::rgb(255, 0, 0)));
    let run = &c.text.as_ref().unwrap().paragraphs[0].runs[0];
    assert!((run.size_pt - 10.0).abs() < 1e-9);
    let ls = lines_of(&sc);
    // the four edges of the one cell, 1 pt black
    assert_eq!(ls.len(), 4, "{ls:?}");
    for l in &ls {
        let line = l.line.as_ref().unwrap();
        assert!((line.width - 12_700.0 * 1.8).abs() < 1.0);
        assert_eq!(line.fill, Fill::Solid(Rgba::BLACK));
    }
}

#[test]
fn a_fill_colour_with_no_fill_kind_is_solid_and_a_background_colour_works() {
    let styles = r##"<style:style style:name="c4" style:family="table-cell"><loext:graphic-properties draw:fill-color="#00ff00"/></style:style>
<style:style style:name="c5" style:family="table-cell"><style:table-cell-properties fo:background-color="#ffff00"/></style:style>
<style:style style:name="c6" style:family="table-cell"><loext:graphic-properties draw:fill="none" draw:fill-color="#00ff00"/></style:style>
<style:style style:name="c7" style:family="table-cell"><loext:graphic-properties draw:fill="solid" draw:fill-color="#808080" draw:opacity="-4900%"/></style:style>
<style:style style:name="c8" style:family="table-cell"><loext:graphic-properties draw:fill="solid" draw:fill-color="#808080" draw:opacity="50%"/></style:style>"##;
    let inner = cols(5)
        + &row(&(cell("c4", "g")
            + &cell("c5", "y")
            + &cell("c6", "n")
            + &cell("c7", "bad")
            + &cell("c8", "half")));
    let sc = scene(&table_op_with("", &inner, styles));
    assert_eq!(rgb(&cell_named(&sc, "g")), Some(Rgba::rgb(0, 255, 0)));
    assert_eq!(rgb(&cell_named(&sc, "y")), Some(Rgba::rgb(255, 255, 0)));
    assert_eq!(cell_named(&sc, "n").fill, Fill::None);
    // an opacity outside 0..100 % is ignored: the fill is opaque
    assert_eq!(rgb(&cell_named(&sc, "bad")).map(|c| c.a), Some(1.0));
    assert_eq!(rgb(&cell_named(&sc, "half")).map(|c| c.a), Some(0.5));
}

#[test]
fn borders_are_read_from_the_cell_properties_and_none_removes_them() {
    let sc = scene(&table_op(&(cols(1) + &rows(1, 1, "c3"))));
    let ls = lines_of(&sc);
    assert_eq!(ls.len(), 1, "{ls:?}");
    let l = &ls[0];
    // the left edge: vertical, at the table's left
    assert!((l.xfrm.x - 2.0 * EMU_CM).abs() < 1.0 && l.xfrm.w == 0.0);
    let line = l.line.as_ref().unwrap();
    assert!((line.width - 25_400.0 * 1.8).abs() < 1.0);
    assert_eq!(line.fill, Fill::Solid(Rgba::rgb(0, 255, 0)));
}

#[test]
fn the_wider_border_wins_on_a_shared_edge_and_none_never_overrides() {
    let styles = r##"<style:style style:name="w1" style:family="table-cell"><style:paragraph-properties fo:border-right="1pt solid #ff0000" fo:border-left="none"/></style:style>
<style:style style:name="w2" style:family="table-cell"><style:paragraph-properties fo:border-left="3pt solid #0000ff" fo:border-right="none"/></style:style>
<style:style style:name="w3" style:family="table-cell"><style:paragraph-properties fo:border-left="none" fo:border-right="none"/></style:style>"##;
    let inner = cols(3) + &row(&(cell("w1", "a") + &cell("w2", "b") + &cell("w3", "c")));
    let sc = scene(&table_op_with("", &inner, styles));
    let ls = lines_of(&sc);
    // a/b share an edge: 1 pt red against 3 pt blue -> blue; b/c: b's right is none, c's left is none
    assert_eq!(ls.len(), 1, "{ls:?}");
    assert_eq!(
        ls[0].line.as_ref().unwrap().fill,
        Fill::Solid(Rgba::rgb(0, 0, 255))
    );
    assert!((ls[0].xfrm.x - 4.0 * EMU_CM).abs() < 1.0);
}

#[test]
fn equal_neighbouring_border_segments_are_one_line() {
    let sc = scene(&table_op(&(cols(3) + &rows(1, 3, "c1"))));
    let ls = lines_of(&sc);
    // top and bottom run across all three cells; four verticals
    let horizontal: Vec<_> = ls.iter().filter(|l| l.xfrm.h == 0.0).collect();
    assert_eq!(horizontal.len(), 2);
    assert!((horizontal[0].xfrm.w - 6.0 * EMU_CM).abs() < 1.0);
    assert_eq!(ls.iter().filter(|l| l.xfrm.w == 0.0).count(), 4);
}

#[test]
fn double_and_dashed_borders() {
    let styles = r##"<style:style style:name="d1" style:family="table-cell"><style:paragraph-properties fo:border="2pt double #000000"/></style:style>
<style:style style:name="d2" style:family="table-cell"><style:paragraph-properties fo:border="1pt dashed #000000"/></style:style>
<style:style style:name="d3" style:family="table-cell"><style:paragraph-properties fo:border="1pt dotted #000000"/></style:style>"##;
    let one = |s: &str| {
        let inner = cols(1) + &row(&cell(s, "x"));
        lines_of(&scene(&table_op_with("", &inner, styles)))[0]
            .line
            .clone()
            .unwrap()
    };
    assert_eq!(one("d1").compound, sd::Compound::Dbl);
    assert_eq!(one("d2").dash, sd::Dash::Dash);
    assert_eq!(one("d3").dash, sd::Dash::SysDot);
}

#[test]
fn unreadable_border_values_are_ignored() {
    let styles = r##"<style:style style:name="b1" style:family="table-cell"><style:paragraph-properties fo:border="" fo:border-left="0pt solid #000000"/></style:style>"##;
    let inner = cols(1) + &row(&cell("b1", "x"));
    assert!(lines_of(&scene(&table_op_with("", &inner, styles))).is_empty());
}

#[test]
fn the_row_default_style_is_used_and_the_cells_own_style_wins() {
    let inner = format!(
        r#"{}<table:table-row table:style-name="ro1" table:default-cell-style-name="c2">{}{}</table:table-row>"#,
        cols(2),
        cell("", "d"),
        cell("c1", "o")
    );
    let sc = scene(&table_op(&inner));
    assert_eq!(rgb(&cell_named(&sc, "d")), Some(Rgba::rgb(0, 0, 255)));
    assert_eq!(rgb(&cell_named(&sc, "o")), Some(Rgba::rgb(255, 0, 0)));
}

#[test]
fn a_default_cell_style_that_is_a_graphic_style_gives_its_fill() {
    // Impress writes `table:default-cell-style-name="standard"`, the drawing style of that name
    let styles = r##"<style:style style:name="standard" style:family="graphic"><style:graphic-properties draw:fill="solid" draw:fill-color="#729fcf"/></style:style>"##;
    let inner = format!(
        r#"{}<table:table-row table:style-name="ro1" table:default-cell-style-name="standard">{}</table:table-row>"#,
        cols(1),
        cell("", "x")
    );
    let sc = scene(&table_op_with("", &inner, styles));
    assert_eq!(
        rgb(&cell_named(&sc, "x")),
        Some(Rgba::rgb(0x72, 0x9f, 0xcf))
    );
}

#[test]
fn padding_anchor_and_vertical_writing_come_from_the_cell_style() {
    let styles = r##"<style:style style:name="p1" style:family="table-cell"><loext:graphic-properties draw:textarea-vertical-align="middle" fo:padding-left="0.5cm" fo:padding-top="0.1cm" fo:padding-right="0.2cm" fo:padding-bottom="0.3cm"/></style:style>
<style:style style:name="p2" style:family="table-cell"><style:table-cell-properties style:vertical-align="bottom" fo:padding="0.4cm" style:writing-mode="tb-rl"/></style:style>
<style:style style:name="p3" style:family="table-cell"/>"##;
    let inner = cols(3) + &row(&(cell("p1", "a") + &cell("p2", "b") + &cell("p3", "c")));
    let sc = scene(&table_op_with("", &inner, styles));
    let body = |t: &str| cell_named(&sc, t).text.unwrap();
    let a = body("a");
    assert_eq!(a.anchor, sd::Anchor::Middle);
    assert!(close(a.insets.0, 0.5 * EMU_CM) && close(a.insets.3, 0.3 * EMU_CM));
    let b = body("b");
    assert_eq!(b.anchor, sd::Anchor::Bottom);
    assert!(close(b.insets.0, 0.4 * EMU_CM) && close(b.insets.1, 0.4 * EMU_CM));
    assert_eq!(b.vert, sd::Vert::EaVert);
    // LibreOffice's own distances when nothing says otherwise
    let c = body("c");
    assert_eq!(c.anchor, sd::Anchor::Top);
    assert!(close(c.insets.0, 90_000.0) && close(c.insets.1, 45_000.0));
}

#[test]
fn the_text_of_a_cell_takes_the_cell_styles_text_and_paragraph_properties() {
    let styles = r##"<style:style style:name="t1" style:family="table-cell"><style:paragraph-properties fo:text-align="end"/><style:text-properties fo:font-size="20pt" fo:font-weight="bold" fo:color="#00ff00"/></style:style>
<style:style style:name="P9" style:family="paragraph"><style:text-properties fo:font-size="12pt"/></style:style>"##;
    let inner = cols(2)
        + &row(&(cell("t1", "a")
            + r#"<table:table-cell table:style-name="t1"><text:p text:style-name="P9">b</text:p></table:table-cell>"#));
    let sc = scene(&table_op_with("", &inner, styles));
    let p = &cell_named(&sc, "a").text.unwrap().paragraphs[0];
    assert_eq!(p.align, sd::Align::Right);
    assert!(p.runs[0].bold && (p.runs[0].size_pt - 20.0).abs() < 1e-9);
    assert_eq!(p.runs[0].fill, Fill::Solid(Rgba::rgb(0, 255, 0)));
    // the paragraph's own style wins over the cell's
    let q = &cell_named(&sc, "b").text.unwrap().paragraphs[0];
    assert!((q.runs[0].size_pt - 12.0).abs() < 1e-9);
}

#[test]
fn automatic_text_is_white_on_a_dark_cell() {
    let styles = r##"<style:style style:name="k1" style:family="table-cell"><loext:graphic-properties draw:fill="solid" draw:fill-color="#101010"/></style:style>"##;
    let inner = cols(1) + &row(&cell("k1", "x"));
    let sc = scene(&table_op_with("", &inner, styles));
    let r = &cell_named(&sc, "x").text.unwrap().paragraphs[0].runs[0];
    assert_eq!(r.fill, Fill::Solid(Rgba::WHITE));
}

#[test]
fn columns_without_a_width_share_what_the_frame_has_left() {
    // the frame is 6 cm wide: 2 cm + two columns of 2 cm each
    let inner = r#"<table:table-column table:style-name="co1"/><table:table-column table:style-name="co2" table:number-columns-repeated="2"/>"#.to_string()
        + &row(&(cell("c1", "a") + &cell("c1", "b") + &cell("c1", "c")));
    let sc = scene(&table_op(&inner));
    assert!(near(at(&cell_named(&sc, "b")), (4.0, 1.0, 2.0, 1.0)));
    assert!(near(at(&cell_named(&sc, "c")), (6.0, 1.0, 2.0, 1.0)));
}

#[test]
fn a_table_without_columns_takes_as_many_as_its_widest_row_and_shares_the_frame() {
    let inner = row(&(cell("c1", "a") + &cell("c1", "b") + &cell("c1", "c")));
    let sc = scene(&table_op(&inner));
    // 6 cm / 3
    assert!(near(at(&cell_named(&sc, "b")), (4.0, 1.0, 2.0, 1.0)));
}

#[test]
fn an_empty_table_draws_nothing() {
    assert!(scene(&table_op(&cols(2))).items.is_empty());
    assert!(scene(&table_op("")).items.is_empty());
}

#[test]
fn the_budgets_cut_rows_columns_and_cells() {
    // too many columns
    let wide = "<table:table-column table:number-columns-repeated=\"1000\"/>".to_string()
        + &row(&cell("c1", "x"));
    let d = doc(&table_op(&wide));
    assert!(d.truncated);
    // too many rows
    let tall = cols(1)
        + "<table:table-row table:number-rows-repeated=\"100000\"><table:table-cell><text:p>x</text:p></table:table-cell></table:table-row>";
    let sc = scene(&table_op(&tall));
    assert!(cells_of(&sc).len() <= super::super::table::MAX_TABLE_ROWS);
    assert!(sc.truncated);
    // rows x columns is capped as well
    let many = "<table:table-column table:number-columns-repeated=\"100\"/>".to_string()
        + "<table:table-row table:number-rows-repeated=\"400\"><table:table-cell table:number-columns-repeated=\"100\"><text:p>x</text:p></table:table-cell></table:table-row>";
    let sc = scene(&table_op(&many));
    assert!(cells_of(&sc).len() <= super::super::table::MAX_TABLE_CELLS);
    assert!(sc.truncated);
}

#[test]
fn the_item_budget_stops_a_table() {
    let opts = DocOptions {
        max_slide_shapes: 5,
        ..DocOptions::default()
    };
    let o = table_op(&(cols(3) + &rows(3, 3, "c1")));
    let d = load_op(&o, &opts).unwrap();
    assert!(d.slide_scenes[0].truncated);
    assert!(shapes_of(&d.slide_scenes[0]).len() <= 5);
}

#[test]
fn a_hostile_span_count_does_not_overflow() {
    let span = r#"<table:table-cell table:number-columns-spanned="18446744073709551615" table:number-rows-spanned="99999999999999999999"><text:p>M</text:p></table:table-cell>"#;
    let inner = cols(2) + &row(&(span.to_string() + &cell("c1", "a")));
    let sc = scene(&table_op(&inner));
    assert!(!cells_of(&sc).is_empty());
}

// ---- templates ---------------------------------------------------------------------------------

const TEMPLATE: &str = r##"
<style:style style:name="body" style:family="table-cell"><loext:graphic-properties draw:fill="solid" draw:fill-color="#99ccff"/></style:style>
<style:style style:name="odd" style:family="table-cell"><loext:graphic-properties draw:fill="solid" draw:fill-color="#0099ff"/></style:style>
<style:style style:name="head" style:family="table-cell"><loext:graphic-properties draw:fill="solid" draw:fill-color="#0066cc"/><style:text-properties fo:color="#ffffff" fo:font-weight="bold"/></style:style>
<style:style style:name="oddcol" style:family="table-cell"><loext:graphic-properties draw:fill="solid" draw:fill-color="#ff9900"/></style:style>
<style:style style:name="evencol" style:family="table-cell"><loext:graphic-properties draw:fill="solid" draw:fill-color="#009900"/></style:style>"##;

fn template_op(attrs: &str, cells_style: &str) -> Op {
    let tpl = r#"<table:table-template table:name="t"><table:first-row table:style-name="head"/><table:last-row table:style-name="head"/><table:first-column table:style-name="head"/><table:last-column table:style-name="head"/><table:body table:style-name="body"/><table:odd-rows table:style-name="odd"/><table:odd-columns table:style-name="oddcol"/><table:even-columns table:style-name="evencol"/></table:table-template>"#;
    let inner = cols(4) + &rows(5, 4, cells_style);
    table_op_with(
        &format!(r#"table:template-name="t" {attrs}"#),
        &inner,
        TEMPLATE,
    )
    .styles(tpl)
}

fn fill_of(sc: &sd::SlideScene, t: &str) -> Option<Rgba> {
    rgb(&cell_named(sc, t))
}

#[test]
fn template_parts_apply_only_when_their_flags_are_on() {
    let sc = scene(&template_op("", ""));
    // no flag: just the body
    assert_eq!(fill_of(&sc, "r0c0"), Some(Rgba::rgb(0x99, 0xcc, 0xff)));
    assert_eq!(fill_of(&sc, "r4c3"), Some(Rgba::rgb(0x99, 0xcc, 0xff)));
}

#[test]
fn template_first_last_row_and_column_and_banding() {
    let sc = scene(&template_op(
        r#"table:use-first-row-styles="true" table:use-last-row-styles="true" table:use-first-column-styles="true" table:use-banding-rows-styles="true""#,
        "",
    ));
    let head = Some(Rgba::rgb(0, 0x66, 0xcc));
    assert_eq!(fill_of(&sc, "r0c2"), head); // first row
    assert_eq!(fill_of(&sc, "r4c2"), head); // last row
    assert_eq!(fill_of(&sc, "r2c0"), head); // first column
                                            // banded rows count from the first body row, which is odd
    assert_eq!(fill_of(&sc, "r1c2"), Some(Rgba::rgb(0, 0x99, 0xff)));
    assert_eq!(fill_of(&sc, "r2c2"), Some(Rgba::rgb(0x99, 0xcc, 0xff)));
    assert_eq!(fill_of(&sc, "r3c2"), Some(Rgba::rgb(0, 0x99, 0xff)));
    // the header's text properties are the cell's text
    let r = &cell_named(&sc, "r0c2").text.unwrap().paragraphs[0].runs[0];
    assert!(r.bold);
    assert_eq!(r.fill, Fill::Solid(Rgba::WHITE));
}

#[test]
fn template_last_column_and_banded_columns() {
    let sc = scene(&template_op(
        r#"table:use-last-column-styles="true" table:use-banding-columns-styles="true""#,
        "",
    ));
    let head = Some(Rgba::rgb(0, 0x66, 0xcc));
    assert_eq!(fill_of(&sc, "r1c3"), head);
    assert_eq!(fill_of(&sc, "r1c0"), Some(Rgba::rgb(0xff, 0x99, 0)));
    assert_eq!(fill_of(&sc, "r1c1"), Some(Rgba::rgb(0, 0x99, 0)));
    assert_eq!(fill_of(&sc, "r1c2"), Some(Rgba::rgb(0xff, 0x99, 0)));
}

#[test]
fn a_cells_own_style_overrides_the_template_property_by_property() {
    let sc = scene(&template_op(r#"table:use-first-row-styles="true""#, "c1"));
    // the explicit fill wins, the template's bold white text stays where the style says nothing
    assert_eq!(fill_of(&sc, "r0c0"), Some(Rgba::rgb(255, 0, 0)));
    let r = &cell_named(&sc, "r0c0").text.unwrap().paragraphs[0].runs[0];
    assert!(r.bold);
}

#[test]
fn an_unknown_template_or_part_is_ignored() {
    let o = table_op_with(
        r#"table:template-name="nope" table:use-first-row-styles="true""#,
        &(cols(1) + &rows(1, 1, "")),
        "",
    );
    let sc = scene(&o);
    assert_eq!(cell_named(&sc, "r0c0").fill, Fill::None);
}

#[test]
fn the_preview_picture_beside_a_table_is_not_drawn() {
    let frame = pic(&format!(
        r#"<table:table table:name="T">{}{}</table:table><draw:image xlink:href="Pictures/TablePreview1.svm"/>"#,
        cols(1),
        rows(1, 1, "c1")
    ));
    let o = op(&slide_page(&frame))
        .auto(&(gr("") + TABLE_STYLES))
        .part("Pictures/TablePreview1.svm", b"VCLMTF");
    assert!(pictures_of(&scene(&o)).is_empty());
}

#[test]
fn the_text_view_of_the_deck_still_has_the_table() {
    let o = table_op(&(cols(2) + &rows(1, 2, "c1")));
    let d = doc(&o);
    assert!(d.markdown.contains("r0c0") && d.markdown.contains("r0c1"));
}

// ---- rows taller than the frame ---------------------------------------------------------------------------------------

const TALL_ROWS: &str = r##"
<style:style style:name="ro4" style:family="table-row"><style:table-row-properties style:row-height="4cm" style:use-optimal-row-height="false"/></style:style>
<style:style style:name="ro5" style:family="table-row"><style:table-row-properties style:min-row-height="4cm"/></style:style>"##;

fn tall_row(style: &str, text: &str) -> String {
    format!(
        r#"<table:table-row table:style-name="{style}"><table:table-cell table:style-name="c2"><text:p>{text}</text:p></table:table-cell></table:table-row>"#
    )
}

#[test]
fn rows_taller_than_the_frame_are_shrunk_to_fit_it() {
    // two rows of 4 cm in the 3 cm frame: each gives up the same share of what it can give
    let inner = format!(
        "{}{}{}",
        cols(1),
        tall_row("ro4", "a"),
        tall_row("ro4", "b")
    );
    let sc = scene(&table_op_with("", &inner, TALL_ROWS));
    let cs = cells_of(&sc);
    let (a, b) = (at(&cs[0]), at(&cs[1]));
    assert!((a.3 - 1.5).abs() < 0.05, "{a:?}");
    assert!((b.3 - 1.5).abs() < 0.05, "{b:?}");
    // together they fill the frame
    assert!((a.3 + b.3 - 3.0).abs() < 0.01);
}

#[test]
fn a_row_is_not_shrunk_below_its_text_or_its_minimum_height() {
    // the text of the first row asks for more than the frame has: it keeps what its text needs
    // and the other row takes the rest of the shrinking, down to the line it is left with
    let long = "word ".repeat(60);
    let inner = format!(
        "{}{}{}",
        cols(1),
        tall_row("ro4", &long),
        tall_row("ro4", "b")
    );
    let cs = cells_of(&scene(&table_op_with("", &inner, TALL_ROWS)));
    let (a, b) = (at(&cs[0]), at(&cs[1]));
    assert!(a.3 > 3.0, "{a:?}");
    assert!(b.3 > 0.5 && b.3 < 4.0, "{b:?}");
    // a minimum height is a floor: nothing is shrunk below it
    let inner = format!(
        "{}{}{}",
        cols(1),
        tall_row("ro5", "a"),
        tall_row("ro5", "b")
    );
    let cs = cells_of(&scene(&table_op_with("", &inner, TALL_ROWS)));
    assert!((at(&cs[0]).3 - 4.0).abs() < 0.01);
    assert!((at(&cs[1]).3 - 4.0).abs() < 0.01);
}

#[test]
fn rows_the_frame_has_room_for_keep_their_height() {
    // 1 cm rows in a 3 cm frame
    let sc = scene(&table_op(&(cols(1) + &rows(2, 1, "c2"))));
    let cs = cells_of(&sc);
    assert!((at(&cs[0]).3 - 1.0).abs() < 0.01);
    assert!((at(&cs[1]).3 - 1.0).abs() < 0.01);
}

#[test]
fn a_row_with_no_text_is_not_shrunk_to_nothing() {
    let empty = r#"<table:table-row table:style-name="ro4"><table:table-cell table:style-name="c2"/></table:table-row>"#;
    let inner = format!("{}{}{}{}", cols(1), empty, empty, empty);
    let cs = cells_of(&scene(&table_op_with("", &inner, TALL_ROWS)));
    for c in &cs {
        assert!(at(c).3 > 0.8, "{:?}", at(c));
    }
}
