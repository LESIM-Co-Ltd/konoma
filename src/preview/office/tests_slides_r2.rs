//! Tests added by the second re-review of the slide previews (PR #37): the work over a whole deck
//! is bounded (parsing and the reading-order pass), a depth limit that drops content says so
//! (`mc:AlternateContent` nested past it, in PowerPoint and in Word), the tie of the reading-order
//! cut is a tie of rounding, a damaged or huge `automatic-styles` costs the OpenDocument master
//! pages nothing, and a slide size of zero is no size.
//!
//! Timings are not tested here (they are in the PR notes); only what a bound leaves in the result.

use super::docx::pptx::*;
use super::docx::*;
use super::slide_order::{reading_order, Kind, Rect, Shape, ROW_TOLERANCE};
use super::tests::{deflated, tmp, write};
use super::tests_docx::{para, tiny_png, Dx};
use super::tests_odp as od;
use super::tests_pptx as px;
use super::tests_pptx::I;

const MIB: u64 = 1024 * 1024;

fn lines(m: &str) -> Vec<&str> {
    m.lines().filter(|l| !l.is_empty()).collect()
}

fn text_box(i: usize, k: usize, text: &str) -> String {
    px::tb(
        (k as i64 % 10) * 800_000,
        (k as i64 / 10) * 1_000_000,
        700_000,
        900_000,
        &px::pa(&format!("s{i} {text}{k}")),
    )
}

// ---------------------------------------------------------------------------------------------
// the work over a whole deck
// ---------------------------------------------------------------------------------------------

#[test]
fn the_defaults_bound_the_work_of_a_deck() {
    let o = DocOptions::default();
    // Parsing runs at 35-40 MB/s: this is about a second, not five.
    assert_eq!(o.max_pptx_read_total, 64 * MIB);
    assert_eq!(o.max_deck_order_shapes, 100_000);
    // The scenes of a deck: 192 MiB of items and 64 MiB of copies of a master's number shapes
    // (the stated bound is their sum), 16 MiB of XML of the layouts and masters kept parsed.
    assert_eq!(o.max_deck_bytes, 192 << 20);
    assert_eq!(o.max_copy_bytes, 64 << 20);
    assert_eq!(o.max_deck_bytes + o.max_copy_bytes, 256 << 20);
    assert_eq!(o.max_master_xml, 16 * MIB);
    assert_eq!(o.max_deck_items, 100_000);
}

#[test]
fn a_big_real_deck_is_read_whole() {
    // 1,000 slides of 50 text boxes (~17 MB of XML, 50,000 shapes): inside both bounds. (The
    // output caps are raised: at the defaults the view stops after about 50 such slides, which is
    // the output budget, not the reading budgets this test is about.)
    let slides: Vec<px::Sl> = (0..1000)
        .map(|i| {
            let shapes: String = (0..50).map(|k| text_box(i, k, "word ")).collect();
            px::sl(&shapes)
        })
        .collect();
    let opts = DocOptions {
        max_markdown_lines: 1_000_000,
        max_markdown_bytes: 64 * 1024 * 1024,
        ..DocOptions::default()
    };
    let d = px::load_px(&px::Px::new(slides), &opts).unwrap();
    assert!(!d.truncated, "{}", &d.markdown[..d.markdown.len().min(300)]);
    assert_eq!(d.slides.len(), 1000);
    assert!(d.markdown.contains("s999 word 49"));
    px::check_headings(&d);
}

#[test]
fn pictures_do_not_count_against_the_xml_read_total() {
    // 300 slides with a 2 MB picture each: one shared, and 33 more distinct ones (68 MB of
    // pictures in all, over the 64 MiB of XML the deck may have): the pictures are not XML parts.
    let big = |seed: u8| {
        let mut v = tiny_png(seed);
        v.resize(2 * 1024 * 1024, 0);
        v
    };
    let mut slides = Vec::new();
    for i in 0..300 {
        let target = if i < 33 {
            format!("../media/d{i}.png")
        } else {
            "../media/shared.png".to_string()
        };
        slides.push(
            px::sl(&px::pic("rIdI", "photo", Some((0, 0, I, I))))
                .rel("rIdI", "image", &target, false),
        );
    }
    let mut p = px::Px::new(slides).media("shared.png", big(200));
    for i in 0..33u8 {
        p = p.media(&format!("d{i}.png"), big(i));
    }
    let d = px::load_px(&p, &DocOptions::default()).unwrap();
    assert!(!d.truncated);
    assert_eq!(d.slides.len(), 300);
    assert_eq!(d.images.len(), 34);
}

#[test]
fn distinct_slides_are_limited_by_the_read_total_too() {
    // Not a repeated slide (which a cache could answer): 60 different slides of silent shapes,
    // and a total for about ten of them. Every slide keeps its place; the rest are cut and said.
    let one = |i: usize| {
        let shapes: String = (0..40)
            .map(|k| px::connector() + &format!("<!--{i}-{k}-->"))
            .collect();
        px::sl(&shapes)
    };
    let p = px::Px::new((0..60).map(one).collect());
    let slide_len = p
        .entries()
        .iter()
        .find(|(n, _)| n == "ppt/slides/slide1.xml")
        .map(|(_, b)| b.len() as u64)
        .unwrap();
    let opts = DocOptions {
        max_pptx_read_total: slide_len * 10,
        ..DocOptions::default()
    };
    let d = px::load_px(&p, &opts).unwrap();
    assert!(d.truncated);
    assert_eq!(d.slides.len(), 60, "every slide keeps its place");
    px::check_headings(&d);
}

#[test]
fn the_reading_order_work_is_bounded_over_the_deck_and_says_so() {
    // 10 slides of 20 text boxes, 100 shapes in all allowed: slide 6 is the one that crosses.
    let slides: Vec<px::Sl> = (0..10)
        .map(|i| px::sl(&(0..20).map(|k| text_box(i, k, "t")).collect::<String>()))
        .collect();
    let p = px::Px::new(slides);
    let opts = DocOptions {
        max_deck_order_shapes: 100,
        ..DocOptions::default()
    };
    let d = px::load_px(&p, &opts).unwrap();
    assert!(d.truncated);
    assert!(d.markdown.contains("s4 t19"), "slide 5 is whole");
    assert!(
        !d.markdown.contains("s5 t0"),
        "slide 6 crossed the bound: no body"
    );
    assert!(!d.markdown.contains("s9 t0"), "the rest has no body");
    // The slides after the bound keep their place and their heading (and their drawing): only the
    // text is left out.
    assert_eq!(d.slides.len(), 10);
    assert_eq!(d.slide_scenes.len(), 10);
    assert_eq!(d.slide_scenes[9].items.len(), 20, "the drawing is whole");
    assert_eq!(
        d.markdown.lines().filter(|l| l.starts_with("## ")).count(),
        10
    );
    px::check_headings(&d);
    // Inside the bound nothing is cut.
    let all = px::load_px(&p, &DocOptions::default()).unwrap();
    assert!(!all.truncated);
    assert_eq!(all.slides.len(), 10);
}

#[test]
fn the_reading_order_bound_counts_the_shapes_of_groups() {
    // One slide of a group of 30 text boxes: 31 shapes through the pass (the group, and 30 inside).
    let inner: String = (0..30).map(|k| text_box(0, k, "g")).collect();
    let g = px::group((0, 0, 9 * I, 7 * I), (0, 0, 9 * I, 7 * I), &inner);
    let p = px::Px::new(vec![px::sl(&g)]);
    let opts = DocOptions {
        max_deck_order_shapes: 20,
        ..DocOptions::default()
    };
    let d = px::load_px(&p, &opts).unwrap();
    assert!(d.truncated);
    let ok = px::load_px(&p, &DocOptions::default()).unwrap();
    assert!(!ok.truncated);
}

#[test]
fn odp_reading_order_work_is_bounded_over_the_deck_and_says_so() {
    let pages: String = (0..10)
        .map(|i| {
            od::page(
                &(0..20)
                    .map(|k| od::tf(k as f64, 0.0, 0.5, 1.0, &format!("s{i} t{k}")))
                    .collect::<String>(),
            )
        })
        .collect();
    let o = od::Op::new(&pages);
    let opts = DocOptions {
        max_deck_order_shapes: 100,
        ..DocOptions::default()
    };
    let d = od::load_op(&o, &opts).unwrap();
    assert!(d.truncated);
    assert!(d.markdown.contains("s4 t19"));
    assert!(!d.markdown.contains("s9 t0"));
    assert_eq!(d.slides.len(), 10, "every slide keeps its heading");
    assert_eq!(d.slide_scenes.len(), 10);
    px::check_headings(&d);
    let all = od::load_op(&o, &DocOptions::default()).unwrap();
    assert!(!all.truncated);
    assert_eq!(all.slides.len(), 10);
}

// ---------------------------------------------------------------------------------------------
// a depth limit that drops content says so
// ---------------------------------------------------------------------------------------------

fn nested_ac_around(inner: &str, depth: usize) -> String {
    let mut s = String::new();
    for _ in 0..depth {
        s += r#"<mc:AlternateContent><mc:Choice Requires="zzz"><p:sp/></mc:Choice><mc:Fallback>"#;
    }
    s += inner;
    for _ in 0..depth {
        s += "</mc:Fallback></mc:AlternateContent>";
    }
    s
}

#[test]
fn pptx_alternate_content_nested_past_the_depth_limit_says_so() {
    let core = px::tb(0, 0, I, I, &px::pa("core"));
    // A few levels are read, and nothing is cut.
    let d = px::doc(&px::Px::new(vec![px::sl(&nested_ac_around(&core, 5))]));
    assert!(d.markdown.contains("core"));
    assert!(!d.truncated);
    // Far past the limit the shape is dropped, and that is said.
    for depth in [25usize, 100] {
        let d = px::doc(&px::Px::new(vec![px::sl(&nested_ac_around(&core, depth))]));
        assert!(d.truncated, "depth {depth}");
        px::check_headings(&d);
    }
    // At every depth around the limit: either the text is there or `truncated` says it is not.
    for depth in 15..30usize {
        let d = px::doc(&px::Px::new(vec![px::sl(&nested_ac_around(&core, depth))]));
        assert!(d.markdown.contains("core") || d.truncated, "depth {depth}");
    }
}

#[test]
fn pptx_inline_alternate_content_nested_past_the_limit_says_so() {
    let nest = |depth: usize| {
        let mut s = String::new();
        for _ in 0..depth {
            s += r#"<mc:AlternateContent><mc:Choice Requires="zzz"><a:r><a:t>no</a:t></a:r></mc:Choice><mc:Fallback>"#;
        }
        s += r#"<a:r><a:t>core</a:t></a:r>"#;
        for _ in 0..depth {
            s += "</mc:Fallback></mc:AlternateContent>";
        }
        px::tb(0, 0, I, I, &format!("<a:p>{s}</a:p>"))
    };
    let d = px::doc(&px::Px::new(vec![px::sl(&nest(3))]));
    assert!(d.markdown.contains("core"));
    assert!(!d.truncated);
    let d = px::doc(&px::Px::new(vec![px::sl(&nest(30))]));
    assert!(!d.markdown.contains("core"));
    assert!(d.truncated);
}

fn docx_nest(depth: usize, inner: &str, wrap_in_run: bool) -> String {
    let mut s = String::new();
    for _ in 0..depth {
        s += r#"<mc:AlternateContent><mc:Choice Requires="zzz"><w:p/></mc:Choice><mc:Fallback>"#;
    }
    s += inner;
    for _ in 0..depth {
        s += "</mc:Fallback></mc:AlternateContent>";
    }
    if wrap_in_run {
        format!("<w:p><w:r>{s}</w:r></w:p>")
    } else {
        s
    }
}

fn load_dx(body: &str) -> Document {
    let dir = tmp("r2docx");
    let p = write(&dir, "t.docx", &Dx::new(body).bytes());
    load_document(&p, &DocOptions::default()).unwrap()
}

#[test]
fn docx_alternate_content_nested_past_the_depth_limit_says_so() {
    let core = para("<w:r><w:t>core</w:t></w:r>");
    // Block level.
    let d = load_dx(&docx_nest(5, &core, false));
    assert!(d.markdown.contains("core") && !d.truncated);
    let d = load_dx(&docx_nest(100, &core, false));
    assert!(d.truncated, "block level: {:?}", d.markdown);
    // Inside a run.
    let d = load_dx(&docx_nest(5, "<w:t>core</w:t>", true));
    assert!(d.markdown.contains("core") && !d.truncated);
    let d = load_dx(&docx_nest(100, "<w:t>core</w:t>", true));
    assert!(d.truncated, "run level: {:?}", d.markdown);
    // Between the runs of a paragraph.
    let inline = docx_nest(100, "<w:r><w:t>core</w:t></w:r>", false);
    let d = load_dx(&para(&inline));
    assert!(d.truncated, "inline level: {:?}", d.markdown);
    // At every depth around the limits: the text is there, or `truncated` says it is not.
    for depth in 35..70 {
        let d = load_dx(&docx_nest(depth, &core, false));
        assert!(d.markdown.contains("core") || d.truncated, "block {depth}");
        let d = load_dx(&docx_nest(depth, "<w:t>core</w:t>", true));
        assert!(d.markdown.contains("core") || d.truncated, "run {depth}");
    }
}

#[test]
fn docx_a_picture_scan_cut_by_the_depth_limit_says_so() {
    // `w:drawing` holding 100 nested `mc:AlternateContent` with a picture at the bottom.
    let mut s = String::from("<w:p><w:r><w:drawing>");
    for _ in 0..100 {
        s += r#"<mc:AlternateContent><mc:Choice Requires="zzz"><a:x/></mc:Choice><mc:Fallback>"#;
    }
    s += r#"<a:blip r:embed="rIdX"/>"#;
    for _ in 0..100 {
        s += "</mc:Fallback></mc:AlternateContent>";
    }
    s += "</w:drawing></w:r></w:p>";
    let d = load_dx(&s);
    assert!(d.truncated);
}

// ---------------------------------------------------------------------------------------------
// the tie of the cut is a tie of rounding
// ---------------------------------------------------------------------------------------------

/// A 2 x 2 grid of cards: `row_gap` between the rows, `col_gap` between the columns; shapes in
/// file order top-left, top-right, bottom-left, bottom-right (0, 1, 2, 3).
fn cards(row_gap: i64, col_gap: i64) -> Vec<Shape> {
    let (w, h) = (3 * I, 2 * I);
    let r = |x: i64, y: i64| Shape {
        rect: Some(Rect::new(x, y, w, h)),
        kind: Kind::Other,
    };
    vec![
        r(0, 0),
        r(w + col_gap, 0),
        r(0, h + row_gap),
        r(w + col_gap, h + row_gap),
    ]
}

const BY_COLUMNS: [usize; 4] = [0, 2, 1, 3];
const BY_ROWS: [usize; 4] = [0, 1, 2, 3];

#[test]
fn equal_gaps_that_differ_by_rounding_are_a_tie_and_columns_win() {
    let g = 274_320; // 0.3 inch
    for d in [0, 1, 2, 100, 5_000, ROW_TOLERANCE] {
        // The gap between rows is the wider by `d` EMU, and the other way round.
        assert_eq!(
            reading_order(&cards(g + d, g), None),
            BY_COLUMNS,
            "row gap wider by {d}"
        );
        assert_eq!(
            reading_order(&cards(g, g + d), None),
            BY_COLUMNS,
            "column gap wider by {d}"
        );
    }
}

#[test]
fn a_gap_wider_by_more_than_the_tolerance_still_wins() {
    let g = 274_320;
    assert_eq!(
        reading_order(&cards(g + ROW_TOLERANCE + 1, g), None),
        BY_ROWS,
        "rows are separated by clearly the wider band"
    );
    assert_eq!(
        reading_order(&cards(g, g + 3 * ROW_TOLERANCE), None),
        BY_COLUMNS
    );
    assert_eq!(
        reading_order(&cards(2 * g, g), None),
        BY_ROWS,
        "twice the gap is a different thing"
    );
}

#[test]
fn the_tie_holds_end_to_end_for_a_card_layout() {
    // 0.3 inch between the cards both ways, stored with a rounding difference of 1 EMU.
    let g = 274_320;
    let s = [
        px::tb(0, 0, 4 * I, I, &px::pa("A")),
        px::tb(4 * I + g + 1, 0, 4 * I, I, &px::pa("B")),
        px::tb(0, I + g, 4 * I, I, &px::pa("C")),
        px::tb(4 * I + g + 1, I + g, 4 * I, I, &px::pa("D")),
    ]
    .concat();
    assert_eq!(lines(&px::md1(&s)), vec!["## Slide 1", "A", "C", "B", "D"]);
    let s = [
        px::tb(0, 0, 4 * I, I, &px::pa("A")),
        px::tb(4 * I + g, 0, 4 * I, I, &px::pa("B")),
        px::tb(0, I + g + 1, 4 * I, I, &px::pa("C")),
        px::tb(4 * I + g, I + g + 1, 4 * I, I, &px::pa("D")),
    ]
    .concat();
    assert_eq!(lines(&px::md1(&s)), vec!["## Slide 1", "A", "C", "B", "D"]);
}

// ---------------------------------------------------------------------------------------------
// OpenDocument master pages
// ---------------------------------------------------------------------------------------------

fn master_page(layout: &str, frames: &str) -> String {
    format!(
        r#"<style:master-page style:name="Default" style:page-layout-name="{layout}">{frames}</style:master-page>"#
    )
}

fn layout_28cm(name: &str) -> String {
    format!(
        r#"<style:page-layout style:name="{name}"><style:page-layout-properties fo:page-width="28cm" fo:page-height="15.75cm"/></style:page-layout>"#
    )
}

#[test]
fn a_huge_automatic_styles_part_costs_the_master_pages_nothing() {
    // More automatic styles than the tree budget (500,000 nodes) allows: the page layouts are
    // read one by one and the rest skipped, so the master pages after them are still read.
    let junk = "<a/>".repeat(520_000);
    let m = master_page("PM1", &od::fr("outline", 1.0, 2.0, 20.0, 5.0, &od::tbx("")));
    let s = od::tf(1.0, 9.0, 5.0, 1.0, "placed")
        + &od::fr_nopos("outline", &od::tbx(&od::tp("from master")));
    let o = od::Op::new(&od::page_with("dp1", "Default", &s))
        .styles_auto(&format!("{junk}{}{junk}", layout_28cm("PM1")))
        .master(&m);
    let d = od::doc(&o);
    assert_eq!(d.markdown, "## Slide 1\n\nfrom master\n\nplaced");
    assert!(!d.truncated);
}

#[test]
fn the_page_size_is_read_among_other_automatic_styles() {
    // The size still comes from the page layout: shapes off the page are read last.
    let s = od::tf(-200.0, 1.0, 5.0, 1.0, "far left")
        + &od::tf(1.0, 12.0, 5.0, 1.0, "low")
        + &od::tf(1.0, 1.0, 5.0, 1.0, "high");
    let auto = format!(
        r#"<style:style style:name="x" style:family="graphic"><style:graphic-properties/></style:style>{}<style:style style:name="y" style:family="graphic"/>"#,
        layout_28cm("PM1")
    );
    let o = od::Op::new(&od::page_with("dp1", "Default", &s))
        .styles_auto(&auto)
        .master(&master_page("PM1", ""));
    assert_eq!(
        od::doc(&o).markdown,
        "## Slide 1\n\nhigh\n\nlow\n\nfar left"
    );
}

#[test]
fn an_oversized_page_layout_costs_only_itself() {
    // PM1 holds more than a layout may; PM2, next to it, is read. A master page of PM2 has the
    // page size, one of PM1 has none (position order).
    let hog = format!(
        r#"<style:page-layout style:name="PM1"><style:page-layout-properties fo:page-width="28cm" fo:page-height="15.75cm"/>{}</style:page-layout>"#,
        "<a/>".repeat(5_000)
    );
    let s = od::tf(-200.0, 1.0, 5.0, 1.0, "far left")
        + &od::tf(1.0, 12.0, 5.0, 1.0, "low")
        + &od::tf(1.0, 1.0, 5.0, 1.0, "high");
    let auto = format!("{hog}{}", layout_28cm("PM2"));
    let known = od::Op::new(&od::page_with("dp1", "Default", &s))
        .styles_auto(&auto)
        .master(&master_page("PM2", ""));
    assert_eq!(
        od::doc(&known).markdown,
        "## Slide 1\n\nhigh\n\nlow\n\nfar left"
    );
    let unknown = od::Op::new(&od::page_with("dp1", "Default", &s))
        .styles_auto(&auto)
        .master(&master_page("PM1", ""));
    assert_eq!(
        od::doc(&unknown).markdown,
        "## Slide 1\n\nfar left\n\nhigh\n\nlow"
    );
}

#[test]
fn a_damaged_automatic_styles_part_still_costs_no_more_than_before() {
    // Broken XML inside the part: the masters are lost (nothing can be read past it), the
    // presentation is not.
    let o = od::Op::new(&od::page_with(
        "dp1",
        "Default",
        &od::tf(1.0, 1.0, 5.0, 1.0, "x"),
    ))
    .styles_auto("<style:style></style:oops>")
    .master(&master_page("PM1", ""));
    assert!(od::doc(&o).markdown.contains('x'));
}

// ---------------------------------------------------------------------------------------------
// a slide size of zero is no size
// ---------------------------------------------------------------------------------------------

fn load_with_size(shapes: &str, size: Option<&str>) -> Document {
    let p = px::Px::new(vec![px::sl(shapes)]);
    let mut e = p.entries();
    for (n, b) in e.iter_mut() {
        if n == "ppt/presentation.xml" {
            let t = String::from_utf8(b.clone()).unwrap();
            let t = t.replace(
                r#"<p:sldSz cx="9144000" cy="6858000"/>"#,
                &size.map(|s| format!("<p:sldSz {s}/>")).unwrap_or_default(),
            );
            *b = t.into_bytes();
        }
    }
    let refs: Vec<(&str, &[u8])> = e.iter().map(|(n, b)| (n.as_str(), b.as_slice())).collect();
    let dir = tmp("r2size");
    let path = write(&dir, "t.pptx", &deflated(&refs));
    load_presentation(&path, &DocOptions::default()).unwrap()
}

#[test]
fn pptx_a_slide_size_of_zero_or_less_is_no_size() {
    // B (first in the file) holds the corner of the slide; A lies above-left of it, off any
    // slide that starts at the corner. With a size of 0 x 0 the corner shape would be "on the
    // slide" and A "off" it: A would come last. With no size known it is the position order.
    let s = px::tb(0, 0, 100, 100, &px::pa("B corner"))
        + &px::tb(-1_000_000, -2_000_000, 500_000, 500_000, &px::pa("A above"));
    let none = load_with_size(&s, None);
    assert_eq!(
        lines(&none.markdown),
        vec!["## Slide 1", "A above", "B corner"]
    );
    for size in [
        r#"cx="0" cy="0""#,
        r#"cx="0" cy="6858000""#,
        r#"cx="9144000" cy="0""#,
        r#"cx="-5" cy="-5""#,
        r#"cx="-9144000" cy="6858000""#,
    ] {
        let d = load_with_size(&s, Some(size));
        assert_eq!(d.markdown, none.markdown, "{size}");
    }
    // A real size still orders what lies off the slide last.
    let real = load_with_size(&s, Some(r#"cx="9144000" cy="6858000""#));
    assert_eq!(
        lines(&real.markdown),
        vec!["## Slide 1", "B corner", "A above"]
    );
}

#[test]
fn odp_a_page_size_of_zero_or_less_is_no_size() {
    let s = od::tf(-1.0, -0.5, 2.0, 1.0, "B corner") + &od::tf(-20.0, -3.0, 1.0, 1.0, "A above");
    let layout = |w: &str, h: &str| {
        format!(
            r#"<style:page-layout style:name="PM1"><style:page-layout-properties fo:page-width="{w}" fo:page-height="{h}"/></style:page-layout>"#
        )
    };
    let with = |auto: Option<String>| {
        let mut o =
            od::Op::new(&od::page_with("dp1", "Default", &s)).master(&master_page("PM1", ""));
        if let Some(a) = auto {
            o = o.styles_auto(&a);
        }
        od::doc(&o).markdown
    };
    let none = with(None);
    assert_eq!(none, "## Slide 1\n\nA above\n\nB corner");
    for (w, h) in [
        ("0cm", "0cm"),
        ("0cm", "15cm"),
        ("28cm", "0cm"),
        ("-28cm", "-15cm"),
    ] {
        assert_eq!(with(Some(layout(w, h))), none, "{w} x {h}");
    }
    // A real size orders what lies off the page last.
    assert_eq!(
        with(Some(layout("28cm", "15.75cm"))),
        "## Slide 1\n\nB corner\n\nA above"
    );
}
