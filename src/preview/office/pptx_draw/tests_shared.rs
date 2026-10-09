//! Tests of what the slides of a deck share: the drawing of a master and of a layout is built once
//! and every slide holds the same list; the deck's item budget; a group with no size.

use std::sync::Arc;

use super::tests::{para, slide, tbox, D, RECT};
use super::*;
use crate::preview::office::slide_draw::{self as sd, Item};

/// `n` text boxes with text `label`, one under the other.
fn boxes(n: usize, label: &str) -> String {
    (0..n)
        .map(|i| {
            tbox(
                100_000,
                10_000 + 1000 * i as i64,
                2_000_000,
                200_000,
                &para(label),
            )
        })
        .collect()
}

fn deck(master: &str, layout: &str, own: &str, slides: usize) -> D {
    let mut d = D::new(own);
    d.master_shapes = master.to_string();
    d.layout_shapes = layout.to_string();
    d.slides = (0..slides).map(|_| slide(own)).collect();
    d
}

fn svg_of(sc: &sd::SlideScene) -> String {
    sd::render_svg(sc, &|_| None).svg
}

fn count(items: &[Item]) -> usize {
    items
        .iter()
        .map(|i| match i {
            Item::Group(g) => 1 + count(&g.items),
            _ => 1,
        })
        .sum()
}

#[test]
fn every_slide_holds_the_same_master_list() {
    let d = deck(&boxes(50, "MASTERMARK"), "", &boxes(2, "own"), 20);
    let doc = d.load();
    assert_eq!(doc.slide_scenes.len(), 20);
    let first = &doc.slide_scenes[0];
    assert_eq!(first.underlay.len(), 1);
    assert_eq!(count(&first.underlay[0]), 50);
    // The slide's own list holds its own shapes only.
    assert_eq!(count(&first.items), 2);
    for sc in &doc.slide_scenes {
        assert_eq!(sc.underlay.len(), 1);
        assert!(Arc::ptr_eq(&sc.underlay[0], &first.underlay[0]));
        assert_eq!(count(&sc.items), 2);
        assert!(!sc.truncated);
    }
    // ... and the picture shows them, under the slide's own.
    let svg = svg_of(first);
    assert_eq!(svg.matches("MASTERMARK").count(), 50, "{svg}");
    assert!(svg.find("MASTERMARK").unwrap() < svg.find("own").unwrap());
}

#[test]
fn the_master_comes_before_the_layout_and_both_before_the_slide() {
    let d = deck(
        &boxes(3, "FROMMASTER"),
        &boxes(2, "FROMLAYOUT"),
        &boxes(1, "FROMSLIDE"),
        4,
    );
    let doc = d.load();
    for sc in &doc.slide_scenes {
        assert_eq!(sc.underlay.len(), 2);
        assert_eq!(count(&sc.underlay[0]), 3);
        assert_eq!(count(&sc.underlay[1]), 2);
        let svg = svg_of(sc);
        let (m, l, s) = (
            svg.find("FROMMASTER").unwrap(),
            svg.find("FROMLAYOUT").unwrap(),
            svg.find("FROMSLIDE").unwrap(),
        );
        assert!(m < l && l < s);
    }
    assert!(Arc::ptr_eq(
        &doc.slide_scenes[0].underlay[1],
        &doc.slide_scenes[3].underlay[1]
    ));
}

#[test]
fn a_layout_that_hides_the_master_shapes_shares_only_its_own() {
    let mut d = deck(&boxes(3, "FROMMASTER"), &boxes(2, "FROMLAYOUT"), "", 3);
    d.layout_attrs = r#" showMasterSp="0""#.to_string();
    let doc = d.load();
    for sc in &doc.slide_scenes {
        assert_eq!(sc.underlay.len(), 1);
        assert_eq!(count(&sc.underlay[0]), 2);
        assert!(!svg_of(sc).contains("FROMMASTER"));
    }
}

#[test]
fn a_slide_that_hides_the_master_shapes_has_no_underlay() {
    let mut d = deck(&boxes(3, "FROMMASTER"), &boxes(2, "FROMLAYOUT"), "", 3);
    d.slides[1].attrs = r#" showMasterSp="0""#.to_string();
    let doc = d.load();
    assert_eq!(doc.slide_scenes[0].underlay.len(), 2);
    assert!(doc.slide_scenes[1].underlay.is_empty());
    assert!(!svg_of(&doc.slide_scenes[1]).contains("FROMLAYOUT"));
    assert_eq!(doc.slide_scenes[2].underlay.len(), 2);
}

fn number_para() -> String {
    r#"<a:p><a:fld id="{00000000-0000-0000-0000-000000000000}" type="slidenum"><a:rPr lang="en-US"/><a:t>NUM</a:t></a:fld></a:p>"#.to_string()
}

#[test]
fn a_master_shape_with_the_slide_number_is_built_for_each_slide_between_the_shared_ones() {
    let master = format!(
        "{}{}{}",
        boxes(2, "MASTERMARK"),
        tbox(0, 0, 900_000, 200_000, &number_para()),
        boxes(1, "AFTERNUMBER")
    );
    let d = deck(&master, &boxes(2, "FROMLAYOUT"), &boxes(1, "own"), 3);
    let doc = d.load();
    let first = &doc.slide_scenes[0];
    // The master's stretch before the number, the number, the stretch after it, the layout.
    assert_eq!(first.underlay.len(), 4);
    for (i, sc) in doc.slide_scenes.iter().enumerate() {
        assert_eq!(sc.underlay.len(), 4, "slide {i}");
        assert!(Arc::ptr_eq(&sc.underlay[0], &first.underlay[0]));
        assert!(Arc::ptr_eq(&sc.underlay[2], &first.underlay[2]));
        assert!(Arc::ptr_eq(&sc.underlay[3], &first.underlay[3]));
        if i > 0 {
            assert!(!Arc::ptr_eq(&sc.underlay[1], &first.underlay[1]));
        }
        assert_eq!(count(&sc.underlay[1]), 1);
        assert_eq!(count(&sc.items), 1);
        let svg = svg_of(sc);
        assert!(svg.contains(&format!(">{}<", i + 1)), "slide {i}: {svg}");
        let at = |s: &str| svg.find(s).unwrap();
        // painted in order: master before the number before what follows it, then the layout
        assert!(at("MASTERMARK") < at(&format!(">{}<", i + 1)));
        assert!(at(&format!(">{}<", i + 1)) < at("AFTERNUMBER"));
        assert!(at("AFTERNUMBER") < at("FROMLAYOUT"));
        assert!(at("FROMLAYOUT") < at("own"));
    }
}

#[test]
fn a_layout_shape_with_the_slide_number_keeps_the_master_shared() {
    let layout = format!(
        "{}{}",
        boxes(1, "FROMLAYOUT"),
        tbox(0, 0, 900_000, 200_000, &number_para())
    );
    let d = deck(&boxes(4, "MASTERMARK"), &layout, "", 3);
    let doc = d.load();
    let first = &doc.slide_scenes[0];
    assert_eq!(first.underlay.len(), 3);
    for (i, sc) in doc.slide_scenes.iter().enumerate() {
        assert!(Arc::ptr_eq(&sc.underlay[0], &first.underlay[0]));
        assert!(Arc::ptr_eq(&sc.underlay[1], &first.underlay[1]));
        assert_eq!(count(&sc.underlay[0]), 4);
        assert_eq!(count(&sc.items), 0, "slide {i}");
        assert!(svg_of(sc).contains(&format!(">{}<", i + 1)));
    }
}

#[test]
fn a_part_of_only_slide_number_shapes_is_all_the_slides_own() {
    let d = deck(&tbox(0, 0, 900_000, 200_000, &number_para()), "", "", 3);
    let doc = d.load();
    for (i, sc) in doc.slide_scenes.iter().enumerate() {
        assert_eq!(sc.underlay.len(), 1);
        assert!(svg_of(sc).contains(&format!(">{}<", i + 1)));
    }
    assert!(!Arc::ptr_eq(
        &doc.slide_scenes[0].underlay[0],
        &doc.slide_scenes[1].underlay[0]
    ));
}

#[test]
fn the_slide_number_shapes_cost_the_deck_for_every_slide() {
    let opts = DocOptions {
        max_deck_items: 10,
        ..DocOptions::default()
    };
    // The master's one shape is the slide's number: built for each slide, 1 item each.
    let d = deck(&tbox(0, 0, 900_000, 200_000, &number_para()), "", "", 15);
    let doc = d.load_with(&opts);
    let shown = doc
        .slide_scenes
        .iter()
        .filter(|s| !s.underlay.is_empty())
        .count();
    assert_eq!(shown, 10);
    assert!(doc.slide_scenes[12].truncated);
    assert!(doc.truncated);
}

#[test]
fn a_slides_colour_map_override_reaches_the_shapes_it_sits_over() {
    let fill = |c: &str| {
        format!(
            r#"<p:sp><p:nvSpPr><p:cNvPr id="2" name="S"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr>{}{RECT}<a:solidFill><a:schemeClr val="{c}"/></a:solidFill></p:spPr></p:sp>"#,
            super::tests::xf(0, 0, 100_000, 100_000)
        )
    };
    let mut d = deck(&fill("bg1"), "", "", 3);
    let swapped = r#"<p:clrMapOvr><a:overrideClrMapping bg1="dk1" tx1="lt1" bg2="dk2" tx2="lt2" accent1="accent1" accent2="accent2" accent3="accent3" accent4="accent4" accent5="accent5" accent6="accent6" hlink="hlink" folHlink="folHlink"/></p:clrMapOvr>"#;
    d.slides[1].ovr = swapped.to_string();
    let doc = d.load();
    let colour = |sc: &sd::SlideScene| match &sc.underlay[0][0] {
        Item::Shape(s) => s.fill.clone(),
        other => panic!("{other:?}"),
    };
    let (a, b, c) = (
        colour(&doc.slide_scenes[0]),
        colour(&doc.slide_scenes[1]),
        colour(&doc.slide_scenes[2]),
    );
    assert_eq!(a, sd::Fill::Solid(sd::Rgba::WHITE));
    assert_eq!(b, sd::Fill::Solid(sd::Rgba::BLACK));
    // The slides seen through the same map still share one list.
    assert!(Arc::ptr_eq(
        &doc.slide_scenes[0].underlay[0],
        &doc.slide_scenes[2].underlay[0]
    ));
    assert!(!Arc::ptr_eq(
        &doc.slide_scenes[0].underlay[0],
        &doc.slide_scenes[1].underlay[0]
    ));
    assert_eq!(a, c);
}

#[test]
fn the_slide_cap_still_counts_the_master_and_the_layout() {
    let opts = DocOptions {
        max_slide_shapes: 10,
        ..DocOptions::default()
    };
    let d = deck(&boxes(6, "m"), &boxes(3, "l"), &boxes(5, "s"), 2);
    let doc = d.load_with(&opts);
    for sc in &doc.slide_scenes {
        // 6 + 3 of the 10 are taken: one of the slide's five fits.
        assert_eq!(count(&sc.items), 1);
        assert!(sc.truncated);
    }
    assert!(doc.truncated);
}

#[test]
fn a_master_over_the_slide_cap_is_cut_and_marked_for_every_slide() {
    let opts = DocOptions {
        max_slide_shapes: 10,
        ..DocOptions::default()
    };
    let d = deck(&boxes(30, "m"), "", &boxes(2, "s"), 3);
    let doc = d.load_with(&opts);
    for sc in &doc.slide_scenes {
        assert_eq!(count(&sc.underlay[0]), 10);
        assert!(sc.items.is_empty());
        assert!(sc.truncated);
    }
}

#[test]
fn the_deck_budget_counts_a_master_once_and_every_slides_own_items() {
    let opts = DocOptions {
        max_deck_items: 100,
        ..DocOptions::default()
    };
    // The master's 60 items are spent once; 40 are left for the slides' own: two slides of 20.
    let d = deck(&boxes(60, "m"), "", &boxes(20, "s"), 5);
    let doc = d.load_with(&opts);
    let own: Vec<usize> = doc.slide_scenes.iter().map(|s| count(&s.items)).collect();
    assert_eq!(own, vec![20, 20, 0, 0, 0]);
    let trunc: Vec<bool> = doc.slide_scenes.iter().map(|s| s.truncated).collect();
    assert_eq!(trunc, vec![false, false, true, true, true]);
    assert!(doc.truncated);
    // Every slide is still there, with its master.
    assert_eq!(doc.slide_scenes.len(), 5);
    assert_eq!(doc.slides.len(), 5);
    assert!(doc.slide_scenes.iter().all(|s| s.underlay.len() == 1));
}

#[test]
fn a_master_that_does_not_fit_the_deck_budget_is_cut_once() {
    let opts = DocOptions {
        max_deck_items: 25,
        ..DocOptions::default()
    };
    let d = deck(&boxes(60, "m"), "", &boxes(5, "s"), 3);
    let doc = d.load_with(&opts);
    for sc in &doc.slide_scenes {
        assert_eq!(count(&sc.underlay[0]), 25);
        assert!(sc.items.is_empty());
        assert!(sc.truncated);
    }
    assert!(doc.truncated);
}

#[test]
fn a_thousand_slides_over_a_big_master_hold_the_master_once() {
    // The shape of the finding: 1,000 slides over a master of 4,500 shapes (2,000 of them kept) was
    // two million items.
    let d = deck(&boxes(MAX_PART_SHAPES, "M"), "", "", 200);
    let doc = d.load();
    assert!(!doc.truncated);
    let master = &doc.slide_scenes[0].underlay[0];
    let unique: std::collections::HashSet<usize> = doc
        .slide_scenes
        .iter()
        .map(|s| Arc::as_ptr(&s.underlay[0]) as usize)
        .collect();
    assert_eq!(unique.len(), 1);
    assert_eq!(master.len(), MAX_PART_SHAPES);
    let held: usize = doc
        .slide_scenes
        .iter()
        .map(|s| count(&s.items))
        .sum::<usize>()
        + master.len();
    assert_eq!(held, MAX_PART_SHAPES);
}

// ---------------------------------------------------------------------------------------------
// a group with no size
// ---------------------------------------------------------------------------------------------

fn xfrm(x: f64, y: f64, w: f64, h: f64) -> sd::Xfrm {
    sd::Xfrm::rect(x, y, w, h)
}

#[test]
fn a_group_map_with_a_zero_child_extent_keeps_that_axis_unscaled() {
    // Both axes: identity scale, moved by the offsets.
    let m = GroupMap::new((10.0, 20.0), (0.0, 0.0), &xfrm(100.0, 200.0, 0.0, 0.0));
    assert_eq!(m.scale, (1.0, 1.0));
    assert_eq!((m.off, m.to), ((10.0, 20.0), (100.0, 200.0)));
    // Only the child extent of one axis is zero: the other is scaled.
    let m = GroupMap::new((0.0, 0.0), (0.0, 50.0), &xfrm(0.0, 0.0, 300.0, 100.0));
    assert_eq!(m.scale, (1.0, 2.0));
    let m = GroupMap::new((0.0, 0.0), (50.0, 0.0), &xfrm(0.0, 0.0, 100.0, 300.0));
    assert_eq!(m.scale, (2.0, 1.0));
    // The group's own extent is zero over a real child extent: members collapse, nothing is NaN.
    let m = GroupMap::new((0.0, 0.0), (50.0, 50.0), &xfrm(0.0, 0.0, 0.0, 100.0));
    assert_eq!(m.scale, (0.0, 2.0));
    let m = GroupMap::new((0.0, 0.0), (50.0, 50.0), &xfrm(0.0, 0.0, 100.0, 0.0));
    assert_eq!(m.scale, (2.0, 0.0));
}

#[test]
fn a_group_map_never_makes_a_non_number() {
    for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -5.0, 0.0] {
        let m = GroupMap::new((0.0, 0.0), (bad, bad), &xfrm(0.0, 0.0, 100.0, 100.0));
        assert_eq!(m.scale, (1.0, 1.0), "child extent {bad}");
    }
    for size in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let m = GroupMap::new((0.0, 0.0), (10.0, 10.0), &xfrm(0.0, 0.0, size, size));
        assert!(m.scale.0.is_finite() && m.scale.1.is_finite(), "ext {size}");
    }
    // A tiny child extent over a huge size does not overflow into infinity.
    let m = GroupMap::new((0.0, 0.0), (1e-300, 1e-300), &xfrm(0.0, 0.0, 1e300, 1e300));
    assert!(m.scale.0.is_finite() && m.scale.1.is_finite());
}

fn group(grp_pr: &str, members: &str) -> String {
    format!(
        r#"<p:grpSp><p:nvGrpSpPr><p:cNvPr id="9" name="G"/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr>{grp_pr}</p:grpSpPr>{members}</p:grpSp>"#
    )
}

fn nothing_is_a_number(sc: &sd::SlideScene) {
    fn walk(items: &[Item]) {
        for i in items {
            match i {
                Item::Shape(s) => {
                    for v in [s.xfrm.x, s.xfrm.y, s.xfrm.w, s.xfrm.h] {
                        assert!(v.is_finite(), "{s:?}");
                    }
                }
                Item::Group(g) => walk(&g.items),
                Item::Picture(p) => {
                    for v in [p.xfrm.x, p.xfrm.y, p.xfrm.w, p.xfrm.h] {
                        assert!(v.is_finite(), "{p:?}");
                    }
                }
            }
        }
    }
    walk(&sc.items);
    let svg = svg_of(sc);
    assert!(!svg.contains("NaN") && !svg.contains("inf"), "{svg}");
}

#[test]
fn groups_with_no_extent_draw_their_members_in_place() {
    let members = tbox(1_000_000, 2_000_000, 500_000, 400_000, &para("MEMBER"));
    // No extent of its own and no child extent; a child extent of zero; only the group's is zero.
    let cases = [
        r#"<a:xfrm><a:off x="100" y="200"/><a:ext cx="0" cy="0"/><a:chOff x="0" y="0"/></a:xfrm>"#,
        r#"<a:xfrm><a:off x="100" y="200"/><a:ext cx="0" cy="0"/><a:chOff x="0" y="0"/><a:chExt cx="0" cy="0"/></a:xfrm>"#,
        r#"<a:xfrm><a:off x="100" y="200"/><a:ext cx="0" cy="900000"/><a:chOff x="0" y="0"/><a:chExt cx="0" cy="900000"/></a:xfrm>"#,
        r#"<a:xfrm><a:off x="100" y="200"/><a:ext cx="800000" cy="0"/><a:chOff x="0" y="0"/><a:chExt cx="400000" cy="400000"/></a:xfrm>"#,
        r#"<a:xfrm><a:off x="100" y="200"/><a:ext cx="0" cy="0"/><a:chOff x="0" y="0"/><a:chExt cx="400000" cy="400000"/></a:xfrm>"#,
    ];
    for (i, c) in cases.iter().enumerate() {
        let sc = D::new(&group(c, &members)).scene();
        nothing_is_a_number(&sc);
        assert!(count(&sc.items) >= 2, "case {i}: {:?}", sc.items);
    }
    // With neither extent the member keeps its size, moved by the group's offset.
    let sc = D::new(&group(cases[0], &members)).scene();
    let Item::Group(g) = &sc.items[0] else {
        panic!("{:?}", sc.items)
    };
    let Item::Shape(s) = &g.items[0] else {
        panic!("{:?}", g.items)
    };
    assert_eq!(
        (s.xfrm.x, s.xfrm.y, s.xfrm.w, s.xfrm.h),
        (1_000_100.0, 2_000_200.0, 500_000.0, 400_000.0)
    );
}
