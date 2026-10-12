//! Moved from `konoma-office-core/src/tests_docx_robust.rs`: the tests that need the host's Markdown renderer.

use super::tests_docx::*;
pub use konoma_office_core::tests_docx_robust::*;

#[test]
fn libreoffice_english_document_through_the_renderer() {
    let Some(d) = read_lo("word.docx") else {
        return;
    };
    let lines = rendered(&d.markdown, 100);
    let all = lines.join("\n");
    assert!(
        all.contains("Word reader sample") && all.contains("Introduction"),
        "{all}"
    );
    assert!(all.contains("• ") || all.contains("- bullet one"), "{all}");
    assert!(
        all.contains("1. first") && all.contains("2. second") && all.contains("3. third"),
        "{all}"
    );
    assert!(
        all.contains('┌') && all.contains("Merged across two columns"),
        "{all}"
    );
    assert!(
        all.contains("footnote¹") && all.contains("endnote²"),
        "{all}"
    );
    assert!(all.contains("1. Footnote text with *star*."), "{all}");
    assert!(
        all.contains("Kept text. This sentence is inserted."),
        "{all}"
    );
    assert!(all.contains("TEXTBOX-TEXT"), "{all}");
    assert!(all.contains("def f(x):"), "{all}");
    // no leftover markup of ours on screen
    // (the picture itself needs the app's image path: here its alt text stands in)
    assert!(!all.contains("<br>") && !all.contains("&#91;"), "{all}");
}

#[test]
fn libreoffice_japanese_document_through_the_renderer() {
    let Some(d) = read_lo("word-ja.docx") else {
        return;
    };
    let all = rendered(&d.markdown, 100).join("\n");
    assert!(
        all.contains("ア. あ") && all.contains("①. 丸数字一"),
        "{all}"
    );
    assert!(
        all.contains("二列にまたがる見出し") && all.contains("縦に結合"),
        "{all}"
    );
    assert!(all.contains("脚注つきの文¹"), "{all}");
}
