//! `O` on a preview made of pages (plugin stage 2 / B3c2, design §10-3-3 layer 2): the list of the
//! pages of a PDF, the sheets of a workbook and the slides of a deck. Pick one, `Enter` goes
//! there. A preview with one page has nothing to list (no hint, a flash), and a Markdown document
//! keeps `O` as its heading outline. Driven through the real `handle_key`.

use super::*;
use crate::keymap::Context;

fn footer_has(s: &Sim, ctx: Context, token: &str) -> bool {
    crate::ops::footer::tokens(&s.app, ctx)
        .iter()
        .any(|t| t == token)
}

fn open_pdf(dir: &crate::test_support::TmpDir, name: &str) -> Sim {
    let mut s = Sim::with_config(&canon(dir), cfg_en()).with_media();
    s.select(name);
    s.enter();
    s.drain_media();
    s
}

#[test]
fn e2e_pagelist_a_pdf_lists_its_pages_and_enter_goes_to_the_chosen_one() {
    let Some(pdf) = sample_path_or_skip("sample.pdf") else {
        return;
    };
    let dir = sandbox("pl_pdf");
    std::fs::copy(&pdf, dir.join("doc.pdf")).unwrap();
    let mut s = open_pdf(&dir, "doc.pdf");
    assert_eq!(s.app.page_indicator(), Some((1, 3)));
    assert!(footer_has(&s, Context::Image, "O:pages"), "the hint");
    s.key('O');
    assert!(s.app.is_outline());
    s.see("Page 1");
    s.see("Page 2");
    s.see("Page 3");
    // The page the preview is on is marked and selected.
    s.see("▶ Page 1");
    assert_eq!(s.app.outline_sel(), 0);
    s.key('j');
    s.enter();
    assert!(!s.app.is_outline());
    s.drain_media();
    assert_eq!(s.app.page_indicator(), Some((2, 3)));
    // Opened again on page 2, `G` + Enter reaches the last page, `q` closes without moving.
    s.key('O');
    assert_eq!(s.app.outline_sel(), 1);
    s.see("▶ Page 2");
    s.key('G');
    s.enter();
    s.drain_media();
    assert_eq!(s.app.page_indicator(), Some((3, 3)));
    s.key('O');
    s.key('k');
    s.key('q');
    assert!(!s.app.is_outline());
    assert_eq!(s.app.page_indicator(), Some((3, 3)));
    // `O` closes it too.
    s.key('O');
    s.key('O');
    assert!(!s.app.is_outline());
}

#[test]
fn e2e_pagelist_a_one_page_pdf_has_nothing_to_list() {
    let dir = sandbox("pl_pdf1");
    std::fs::write(
        dir.join("one.pdf"),
        konoma_test_support::build_minimal_one_page_pdf(200, 200, b"0 0 0 rg 0 0 100 100 re f"),
    )
    .unwrap();
    let mut s = open_pdf(&dir, "one.pdf");
    assert!(!footer_has(&s, Context::Image, "O:pages"));
    s.key('O');
    assert!(!s.app.is_outline());
    s.see("no other page to go to");
}

#[test]
fn e2e_pagelist_a_workbook_lists_its_sheets_by_name() {
    let dir = sandbox("pl_sheets");
    three_sheet_book(&dir.join("t.xlsx"), "v1");
    let mut s = Sim::with_config(&canon(&dir), cfg_en()).with_media();
    s.select("t.xlsx");
    s.enter();
    s.drain_media();
    assert!(footer_has(&s, Context::Table, "O:pages"), "the hint");
    s.key('O');
    assert!(s.app.is_outline());
    for name in ["One", "Two", "Three"] {
        s.see(name);
    }
    s.see("▶ One");
    s.key('j');
    s.key('j');
    s.enter();
    s.drain_media();
    assert!(!s.app.is_outline());
    s.see("Three (3/3)");
    s.key('O');
    assert_eq!(s.app.outline_sel(), 2);
    s.key('q');
}

#[test]
fn e2e_pagelist_a_one_sheet_workbook_has_nothing_to_list() {
    let dir = sandbox("pl_sheet1");
    build_xlsx(
        &dir.join("o.xlsx"),
        &[(
            "Only",
            "visible",
            &format!(r#"<row r="1">{}</row>"#, x_str("A1", "x")),
            "",
        )],
    );
    let mut s = Sim::with_config(&canon(&dir), cfg_en()).with_media();
    s.select("o.xlsx");
    s.enter();
    s.drain_media();
    assert!(!footer_has(&s, Context::Table, "O:pages"));
    s.key('O');
    assert!(!s.app.is_outline());
    s.see("no other page to go to");
}

#[test]
fn e2e_pagelist_a_markdown_document_keeps_the_heading_outline() {
    let dir = sandbox("pl_md");
    std::fs::write(dir.join("n.md"), "# One\n\ntext\n\n## Two\n\nmore\n").unwrap();
    let mut s = Sim::with_config(&canon(&dir), cfg_en());
    s.select("n.md");
    s.enter();
    assert!(footer_has(&s, Context::Document, "O:outline"));
    assert!(!footer_has(&s, Context::Document, "O:pages"));
    s.key('O');
    assert!(s.app.is_outline());
    s.see("Outline");
    s.see("• One");
    s.dont_see("Pages");
}
