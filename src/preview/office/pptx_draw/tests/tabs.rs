//! Tabs through the reader: the tab character is kept in the run, `a:tabLst` and `defTabSz` are
//! inherited like the other paragraph properties.

use super::*;

fn first_para(shapes: &str, d_lst: &str) -> sd::Paragraph {
    let d = D::new(&shape(
        "",
        &format!("{}{RECT}", xf(0, 0, 4_000_000, 1_000_000)),
        "",
        d_lst,
        shapes,
        "",
    ));
    let sc = d.scene();
    first_shape(&sc).text.as_ref().unwrap().paragraphs[0].clone()
}

#[test]
fn a_tab_in_the_text_is_kept_and_other_whitespace_is_still_cleaned() {
    let p = first_para(&para("a\tb\nc"), "");
    assert_eq!(p.runs.len(), 1);
    assert_eq!(p.runs[0].text, "a\tb c");
    // a run that is only a tab survives (it is not "empty")
    let d = D::new(&shape(
        "",
        &format!("{}{RECT}", xf(0, 0, 4_000_000, 1_000_000)),
        "",
        "",
        &format!("{}{}", para("x"), para("\t")),
        "",
    ));
    let sc = d.scene();
    let ps = &first_shape(&sc).text.as_ref().unwrap().paragraphs;
    assert_eq!(ps[1].runs[0].text, "\t");
}

#[test]
fn tab_stops_and_the_default_distance_are_read() {
    let ppr = r#"<a:pPr defTabSz="457200"><a:tabLst><a:tab pos="100000" algn="l"/><a:tab pos="200000" algn="ctr"/><a:tab pos="300000" algn="r"/><a:tab pos="400000" algn="dec"/><a:tab pos="x" algn="l"/><a:tab algn="l"/></a:tabLst></a:pPr>"#;
    let p = first_para(&para_with(ppr, "", "t"), "");
    assert_eq!(p.def_tab, 457_200.0);
    assert_eq!(
        p.tabs,
        vec![
            sd::TabStop {
                pos: 100_000.0,
                align: sd::TabAlign::Left
            },
            sd::TabStop {
                pos: 200_000.0,
                align: sd::TabAlign::Center
            },
            sd::TabStop {
                pos: 300_000.0,
                align: sd::TabAlign::Right
            },
            sd::TabStop {
                pos: 400_000.0,
                align: sd::TabAlign::Decimal
            },
        ]
    );
}

#[test]
fn without_tab_properties_the_default_is_one_inch_and_no_stops() {
    let p = first_para(&para("t"), "");
    assert_eq!(p.def_tab, sd::DEFAULT_TAB_EMU);
    assert!(p.tabs.is_empty());
}

#[test]
fn tab_properties_come_from_the_list_style_and_the_paragraph_wins() {
    let lst = r#"<a:lvl1pPr defTabSz="228600"><a:tabLst><a:tab pos="50000" algn="l"/></a:tabLst></a:lvl1pPr>"#;
    let p = first_para(&para("t"), lst);
    assert_eq!(p.def_tab, 228_600.0);
    assert_eq!(p.tabs.len(), 1);
    let own = para_with(r#"<a:pPr defTabSz="100000"/>"#, "", "t");
    let p = first_para(&own, lst);
    assert_eq!(p.def_tab, 100_000.0);
    // the list style's stops still apply (the paragraph states none)
    assert_eq!(p.tabs.len(), 1);
}

#[test]
fn a_non_positive_default_distance_is_ignored() {
    let p = first_para(&para_with(r#"<a:pPr defTabSz="0"/>"#, "", "t"), "");
    assert_eq!(p.def_tab, sd::DEFAULT_TAB_EMU);
}

#[test]
fn the_number_of_stops_is_bounded() {
    let many: String = (0..100)
        .map(|i| format!(r#"<a:tab pos="{}" algn="l"/>"#, i * 1000))
        .collect();
    let ppr = format!("<a:pPr><a:tabLst>{many}</a:tabLst></a:pPr>");
    let p = first_para(&para_with(&ppr, "", "t"), "");
    assert_eq!(p.tabs.len(), 32);
}
