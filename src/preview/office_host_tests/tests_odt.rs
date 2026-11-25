//! Moved from `konoma-office-core/src/tests_odt.rs`: the tests that need the host's Markdown renderer.

use super::tests_docx::visible;
pub use konoma_office_core::tests_odt::*;

#[test]
fn a_numbered_heading_label_is_part_of_its_anchor() {
    let styles = outline_style(
        r#"<text:outline-level-style text:level="1" style:num-format="1" style:num-suffix="."/>"#,
    );
    let body = r##"<text:h text:outline-level="1">Intro</text:h><text:p><text:a xlink:href="#Intro|outline">go</text:a></text:p>"##;
    let m = conv(&Ox::new(body).styles(&styles)).markdown;
    // The slug is built from the label and the text, as the renderer builds it for the heading.
    assert!(m.contains("[go](#1-intro)"), "{m}");
    let shown = visible(&m);
    assert!(shown.contains("1. Intro"), "{shown}");
}

#[test]
fn the_converted_odt_draws_through_konomas_markdown_renderer() {
    let Some(d) = read_lo("word.odt") else {
        return;
    };
    let shown = visible(&d.markdown);
    for want in [
        "Word reader sample",
        "Introduction",
        "- bullet one",
        "1. first",
        "a. alpha",
        "Merged across two columns",
        "TEXTBOX-TEXT inside a frame",
    ] {
        assert!(shown.contains(want), "{want:?} in\n{shown}");
    }
    assert!(!shown.contains("# Introduction"), "{shown}");
}
