//! A deck whose theme names its typefaces with thousands of bytes, used by many runs: the names
//! must be shared (cut short, one allocation) and the deck budget must see what the runs hold.

use std::sync::Arc;

use super::tests::{tbox, D, THEME};
use crate::preview::office::slide_draw::footprint::items_bytes;
use crate::preview::office::slide_draw::strings::MAX_NAME_BYTES;
use crate::preview::office::slide_draw::{self as sd, Item};

/// Runs of one paragraph, each asking for the theme's minor typefaces (Latin, East Asian, complex).
fn runs(n: usize) -> String {
    let run = r#"<a:r><a:rPr lang="en-US"><a:latin typeface="+mn-lt"/><a:ea typeface="+mn-ea"/><a:cs typeface="+mn-cs"/></a:rPr><a:t>x</a:t></a:r>"#;
    format!("<a:p>{}</a:p>", run.repeat(n))
}

/// The theme with its minor typefaces replaced by names of `len` bytes (distinct per script).
fn theme_with_names(len: usize) -> String {
    let name = |c: char| c.to_string().repeat(len);
    THEME.replace(
        r#"<a:minorFont><a:latin typeface="Calibri"/><a:ea typeface=""/><a:cs typeface=""/>"#,
        &format!(
            r#"<a:minorFont><a:latin typeface="{}"/><a:ea typeface="{}"/><a:cs typeface="{}"/>"#,
            name('l'),
            name('e'),
            name('c')
        ),
    )
}

fn deck(len: usize, n: usize) -> sd::SlideScene {
    let mut d = D::new(&tbox(0, 0, 1000, 1000, &runs(n)));
    d.theme = Some(theme_with_names(len));
    d.scene()
}

fn all_runs(items: &[Item]) -> Vec<&sd::Run> {
    let mut out = Vec::new();
    for i in items {
        match i {
            Item::Shape(s) => {
                for p in s.text.iter().flat_map(|t| &t.paragraphs) {
                    out.extend(p.runs.iter());
                }
            }
            Item::Group(g) => out.extend(all_runs(&g.items)),
            Item::Picture(_) => {}
        }
    }
    out
}

#[test]
fn a_huge_typeface_name_is_cut_and_one_allocation_serves_every_run() {
    let sc = deck(4000, 3000);
    let runs = all_runs(&sc.items);
    assert!(runs.len() >= 3000, "{}", runs.len());
    for pick in [
        (|r: &sd::Run| r.font.latin.clone()) as fn(&sd::Run) -> Option<Arc<str>>,
        |r| r.font.east_asian.clone(),
        |r| r.font.complex.clone(),
    ] {
        let first = pick(runs[0]).expect("a name");
        assert_eq!(first.len(), MAX_NAME_BYTES);
        for r in &runs {
            assert!(Arc::ptr_eq(&first, &pick(r).unwrap()));
        }
    }
}

#[test]
fn the_estimate_of_a_deck_with_huge_names_is_the_estimate_of_one_with_short_names() {
    let n = 3000;
    let short = items_bytes(&deck(8, n).items);
    let huge = items_bytes(&deck(4000, n).items);
    // What the runs hold differs by the names only (a few dozen bytes once, not per run):
    // before the names were cut and shared, 3 x 4 KB per run were held and counted as nothing.
    assert!(huge <= short + 4096, "{huge} vs {short}");
    assert!(huge >= short, "{huge} vs {short}");
    // The estimate is of the order of what the runs themselves take.
    assert!(huge >= n * std::mem::size_of::<sd::Run>());
    assert!(
        huge < n * (std::mem::size_of::<sd::Run>() + 256),
        "{huge}: the names must not be in it per run"
    );
}
