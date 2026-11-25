//! Moved from `konoma-office-core/src/tests_pptx.rs`: the tests that need the host's Markdown renderer.

use super::tests_docx::rendered;
pub use konoma_office_core::tests_pptx::*;

#[test]
fn the_renderer_shows_slides_as_headings_bullets_and_a_quote() {
    let sld = sl(&[
        title("Plan"),
        body((0, 2 * I, 4 * I, 2 * I), &(pa("first") + &pl(1, "nested"))),
    ]
    .concat())
    .notes(&notes_body(&pa("say hello")));
    let d = doc(&Px::new(vec![sld, sl(&title("Next")).hidden()]));
    let shown = rendered(&d.markdown, 80).join("\n");
    for want in [
        "Slide 1: Plan",
        "first",
        "nested",
        "Notes",
        "say hello",
        "Slide 2: Next (hidden)",
    ] {
        assert!(shown.contains(want), "{want:?} in\n{shown}");
    }
    assert!(!shown.contains("**"), "{shown}");
    assert!(!shown.contains("## "), "{shown}");
}

#[test]
fn the_renderer_shows_a_table_and_an_image_placeholder() {
    let s = [
        table((0, 0, I, I), &(tr(&["A", "B"]) + &tr(&["1", "2"]))),
        pic("rIdMissing", "chart pic", Some((0, 3 * I, I, I))),
    ]
    .concat();
    let d = doc(&Px::new(vec![sl(&s)]));
    let shown = rendered(&d.markdown, 80).join("\n");
    assert!(shown.contains('A') && shown.contains('2'), "{shown}");
    assert!(shown.contains("[chart pic]"), "{shown}");
}
