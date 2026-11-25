//! Moved from `konoma-office-core/src/tests_pptx_real.rs`: the tests that need the host's Markdown renderer.

use super::test_labels::Lang;
use super::tests_docx::rendered;
pub use konoma_office_core::tests_pptx_real::*;

#[test]
fn the_decks_render() {
    for (name, lang) in [("slides.pptx", Lang::En), ("slides-ja.pptx", Lang::Jp)] {
        let Some(d) = load(name, lang) else {
            continue;
        };
        let shown = rendered(&d.markdown, 100).join("\n");
        assert!(!shown.contains("**"), "{shown}");
        assert!(!shown.contains("## "), "{shown}");
        assert!(
            shown.contains("the site") || shown.contains("サイト"),
            "{shown}"
        );
    }
}
