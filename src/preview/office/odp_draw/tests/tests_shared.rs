//! Tests of what the slides of an OpenDocument deck share: a master page's drawing is built once
//! per footer line and page colour, the deck's item budget, the page count in a shared drawing.

use std::sync::Arc;

use super::*;
use crate::preview::office::docx::DocOptions;

fn master_rects(n: usize) -> String {
    (0..n)
        .map(|i| {
            format!(
                r#"<draw:rect draw:style-name="gr1" svg:x="{i}mm" svg:y="0cm" svg:width="5mm" svg:height="5mm"/>"#
            )
        })
        .collect()
}

fn op_with_master(pages: &str, master: &str, dp: &str) -> Op {
    Op::new(pages)
        .styles_auto(LAYOUT)
        .master(&master_with(master, MDP))
        .styles(r#"<style:style style:name="Mdp1" style:family="drawing-page"/>"#)
        .auto(&format!("{}{dp}", filled()))
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

fn pages(n: usize, inner: &str) -> String {
    (0..n).map(|_| slide_page(inner)).collect()
}

#[test]
fn every_slide_holds_the_same_master_list() {
    let o = op_with_master(&pages(20, &rect("")), &master_rects(40), "");
    let ds = scenes(&o);
    assert_eq!(ds.len(), 20);
    let first = &ds[0];
    assert_eq!(first.underlay.len(), 1);
    assert_eq!(count(&first.underlay[0]), 40);
    for sc in &ds {
        assert_eq!(sc.underlay.len(), 1);
        assert!(Arc::ptr_eq(&sc.underlay[0], &first.underlay[0]));
        // (The slide's own list holds only the slide's own rectangle.)
        assert_eq!(count(&sc.items), 1);
        assert!(!sc.truncated);
    }
    // The picture shows the master's, under the slide's.
    let svg = sd::render_svg(first, &|_| None).svg;
    assert!(
        svg.matches("<rect").count() + svg.matches("<path").count() >= 41,
        "{svg}"
    );
}

#[test]
fn slides_with_different_footer_lines_have_their_own_drawing_of_the_master() {
    let dp = |n: u8, footer: &str| {
        format!(
            r#"<style:style style:name="dp{n}" style:family="drawing-page"><style:drawing-page-properties presentation:display-footer="true" presentation:display-page-number="false" presentation:use-footer-name="{footer}"/></style:style>"#
        )
    };
    let decls = r#"<presentation:footer-decl presentation:name="fa">Alpha</presentation:footer-decl><presentation:footer-decl presentation:name="fb">Beta</presentation:footer-decl>"#;
    let page = |style: &str| {
        format!(
            r#"<draw:page draw:name="p" draw:style-name="{style}" draw:master-page-name="Default"/>"#
        )
    };
    let body = format!("{decls}{}{}{}", page("dp1"), page("dp2"), page("dp1"));
    let o = Op::new(&body)
        .styles_auto(LAYOUT)
        .master(&master_with(&master_shapes(), MDP))
        .styles(r#"<style:style style:name="Mdp1" style:family="drawing-page"/>"#)
        .auto(&format!("{}{}{}", filled(), dp(1, "fa"), dp(2, "fb")));
    let ds = scenes(&o);
    assert!(Arc::ptr_eq(&ds[0].underlay[0], &ds[2].underlay[0]));
    assert!(!Arc::ptr_eq(&ds[0].underlay[0], &ds[1].underlay[0]));
    let texts = |sc: &sd::SlideScene| -> Vec<String> {
        shapes_of(sc).iter().map(|s| text_of_shape(s)).collect()
    };
    assert!(texts(&ds[0]).contains(&"Alpha".to_string()));
    assert!(texts(&ds[1]).contains(&"Beta".to_string()));
    assert!(texts(&ds[2]).contains(&"Alpha".to_string()));
}

#[test]
fn a_master_with_the_page_number_builds_only_that_shape_for_each_slide() {
    let dp = r#"<style:style style:name="dp1" style:family="drawing-page"><style:drawing-page-properties presentation:display-page-number="true"/></style:style>"#;
    let o = master_op(&pages(3, &rect("")), dp);
    let ds = scenes(&o);
    for (i, sc) in ds.iter().enumerate() {
        // The bar and the footer before the number are shared, the number is the slide's own.
        assert_eq!(sc.underlay.len(), 2, "slide {i}");
        assert!(Arc::ptr_eq(&sc.underlay[0], &ds[0].underlay[0]));
        if i > 0 {
            assert!(!Arc::ptr_eq(&sc.underlay[1], &ds[0].underlay[1]));
        }
        let texts: Vec<String> = shapes_of(sc).iter().map(|s| text_of_shape(s)).collect();
        assert!(texts.contains(&(i + 1).to_string()), "{texts:?}");
        assert_eq!(count(&sc.underlay[1]), 1);
    }
}

#[test]
fn a_page_name_field_in_the_master_is_the_slides_own() {
    let master = r#"<draw:frame svg:x="1cm" svg:y="1cm" svg:width="5cm" svg:height="1cm"><draw:text-box><text:p><text:page-name>?</text:page-name></text:p></draw:text-box></draw:frame>"#;
    let named = |n: &str| {
        format!(
            r#"<draw:page draw:name="{n}" draw:style-name="dp1" draw:master-page-name="Default"/>"#
        )
    };
    let o = op_with_master(&format!("{}{}", named("Alpha"), named("Beta")), master, "");
    let ds = scenes(&o);
    assert!(ds.iter().all(|s| s.underlay.len() == 1));
    assert!(!Arc::ptr_eq(&ds[0].underlay[0], &ds[1].underlay[0]));
    for (sc, name) in ds.iter().zip(["Alpha", "Beta"]) {
        let texts: Vec<String> = shapes_of(sc).iter().map(|s| text_of_shape(s)).collect();
        assert!(texts.contains(&name.to_string()), "{texts:?}");
    }
}

#[test]
fn a_shape_between_two_numbered_ones_keeps_its_place() {
    let num = r#"<draw:frame svg:x="1cm" svg:y="1cm" svg:width="5cm" svg:height="1cm"><draw:text-box><text:p><text:page-number>?</text:page-number></text:p></draw:text-box></draw:frame>"#;
    let master = format!(
        "{}{num}{}{num}{}",
        master_rects(2),
        master_rects(3),
        master_rects(1)
    );
    let o = op_with_master(&pages(2, ""), &master, "");
    for sc in scenes(&o) {
        let lens: Vec<usize> = sc.underlay.iter().map(|u| count(u)).collect();
        assert_eq!(lens, vec![2, 1, 3, 1, 1]);
    }
}

#[test]
fn the_page_count_is_patched_in_the_shared_list_once() {
    let master = r#"<draw:frame svg:x="1cm" svg:y="1cm" svg:width="5cm" svg:height="1cm"><draw:text-box><text:p><text:page-count>?</text:page-count></text:p></draw:text-box></draw:frame>"#;
    let o = op_with_master(&pages(7, &rect("")), master, "");
    let ds = scenes(&o);
    for sc in &ds {
        assert!(Arc::ptr_eq(&sc.underlay[0], &ds[0].underlay[0]));
        let texts: Vec<String> = shapes_of(sc).iter().map(|s| text_of_shape(s)).collect();
        assert!(texts.contains(&"7".to_string()), "{texts:?}");
        assert!(texts.iter().all(|t| !t.contains(body::PAGE_COUNT_MARK)));
    }
}

#[test]
fn the_page_background_items_stay_under_the_master() {
    let layout = r#"<style:page-layout style:name="PM1"><style:page-layout-properties fo:page-width="28cm" fo:page-height="15.75cm" fo:margin-left="2cm" fo:margin-top="2cm" fo:margin-right="1cm" fo:margin-bottom="0cm"/></style:page-layout>"#;
    let dp = r##"<style:style style:name="dp1" style:family="drawing-page"><style:drawing-page-properties draw:background-size="border" draw:fill="solid" draw:fill-color="#729fcf"/></style:style>"##;
    let o = op_with_master(&pages(2, ""), &master_rects(3), dp).styles_auto(layout);
    for sc in scenes(&o) {
        // The coloured rectangle inside the margins first, then the master's three.
        assert_eq!(sc.underlay.len(), 2);
        assert_eq!(count(&sc.underlay[0]), 1);
        assert_eq!(count(&sc.underlay[1]), 3);
        let Item::Shape(s) = &sc.underlay[0][0] else {
            panic!("{:?}", sc.underlay[0])
        };
        assert_eq!(s.fill, Fill::Solid(Rgba::rgb(0x72, 0x9f, 0xcf)));
    }
}

#[test]
fn the_deck_budget_counts_a_master_once_and_every_slides_own_items() {
    let opts = DocOptions {
        max_deck_items: 100,
        ..DocOptions::default()
    };
    // The master's 60 items are spent once; 40 are left for the slides: two slides of 20.
    let own: String = (0..20).map(|_| rect("")).collect();
    let o = op_with_master(&pages(5, &own), &master_rects(60), "");
    let d = load_op(&o, &opts).unwrap();
    let kept: Vec<usize> = d.slide_scenes.iter().map(|s| count(&s.items)).collect();
    assert_eq!(kept, vec![20, 20, 0, 0, 0]);
    let cut: Vec<bool> = d.slide_scenes.iter().map(|s| s.truncated).collect();
    assert_eq!(cut, vec![false, false, true, true, true]);
    assert!(d.truncated);
    assert_eq!(d.slides.len(), 5);
    assert!(d.slide_scenes.iter().all(|s| s.underlay.len() == 1));
}

#[test]
fn the_slide_cap_still_counts_the_master() {
    let opts = DocOptions {
        max_slide_shapes: 10,
        ..DocOptions::default()
    };
    let own: String = (0..5).map(|_| rect("")).collect();
    let o = op_with_master(&pages(2, &own), &master_rects(6), "");
    let d = load_op(&o, &opts).unwrap();
    for sc in &d.slide_scenes {
        assert_eq!(count(&sc.underlay[0]), 6);
        assert_eq!(count(&sc.items), 4);
        assert!(sc.truncated);
    }
}

#[test]
fn a_thousand_slides_over_a_big_master_hold_the_master_once() {
    // The shape of the finding: 1,000 slides over a master of 4,500 shapes was 4.5 million items.
    let o = op_with_master(&pages(200, ""), &master_rects(4_500), "");
    let ds = scenes(&o);
    assert_eq!(ds.len(), 200);
    let unique: std::collections::HashSet<usize> = ds
        .iter()
        .map(|s| Arc::as_ptr(&s.underlay[0]) as usize)
        .collect();
    assert_eq!(unique.len(), 1);
    assert_eq!(ds[0].underlay[0].len(), 4_500);
    assert!(ds.iter().all(|s| s.items.is_empty()));
}
