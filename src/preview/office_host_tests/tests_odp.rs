//! Moved from `konoma-office-core/src/tests_odp.rs`: the tests that need the host's Markdown renderer.

use super::tests_pptx::check_headings;
pub use konoma_office_core::tests_odp::*;

#[test]
fn markdown_looking_text_is_escaped() {
    let ps = [
        "# h1",
        "## h2",
        "- dash",
        "1. one",
        "> quote",
        "---",
        "***",
        "```",
        "| a | b |",
        "[x](y)",
        "![i](j)",
        "$x$ and $$y$$",
        "*em* _u_ ~~s~~",
        "Setext\n===",
    ];
    let t: String = ps.iter().map(|p| tp(&p.replace('\n', " "))).collect();
    let d = doc(&Op::new(&page(
        &(title("T") + &fr("", 1.0, 3.0, 10.0, 10.0, &tbx(&t))),
    )));
    check_headings(&d);
    assert_eq!(heading_lines(&d).len(), 1, "{}", d.markdown);
    let shown = super::tests_docx::visible(&d.markdown);
    for p in ps {
        let p = p.replace('\n', " ");
        assert!(shown.contains(p.as_str()), "{p:?} not shown in\n{shown}");
    }
}
