//! Moved from `konoma-office-core/src/tests_docx.rs`: the tests that need the host's Markdown renderer.

use crate::preview::markdown::{render_markdown, CodeStyle};
pub use konoma_office_core::tests_docx::*;

/// The text konoma's renderer shows for a Markdown string (width 200, lines joined by `\n`,
/// trailing blanks trimmed).
pub(super) fn visible(markdown: &str) -> String {
    rendered(markdown, 200)
        .iter()
        .map(|l| l.trim_end().to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Renders the way the app does (front matter, footnotes, inline HTML, then the block renderer).
pub(super) fn rendered(src: &str, width: u16) -> Vec<String> {
    use crate::preview::markdown::{process_footnotes, process_inline_html, strip_front_matter};
    let body = strip_front_matter(src).1;
    let pre = process_inline_html(&process_footnotes(&body));
    render_markdown(&pre, width, CodeStyle::default(), "TwoDark", false)
        .iter()
        .map(|l| {
            l.spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>()
        })
        .collect()
}

#[test]
fn emphasis_ending_in_punctuation_before_a_word_still_renders() {
    // CommonMark: `**done.**next` is not emphasis (the closer follows punctuation and precedes a
    // letter); the reader moves the punctuation out so the bold survives.
    let body = para(
        &(runp("<w:b/>", "日本語。")
            + &run("次の文")
            + &run(" and ")
            + &runp("<w:b/>", "「引用」")
            + &run("次")),
    );
    let m = md(&body);
    assert_eq!(
        m,
        "**日本語**。次の文 and 「**引用**」次".replace("「**引用**」", "**「引用**」")
    );
    assert_eq!(visible(&m), "日本語。次の文 and 「引用」次");
}

#[test]
fn emphasis_starting_with_punctuation_after_a_word_still_renders() {
    let body = para(&(run("前") + &runp("<w:b/>", "「太字」")));
    let m = md(&body);
    assert_eq!(visible(&m), "前「太字」");
    assert!(m.contains("**"), "{m}");
    assert!(!visible(&m).contains('*'), "{m}");
}

#[test]
fn a_line_break_is_a_hard_break() {
    let body =
        para(&(run("a") + "<w:r><w:br/></w:r>" + &run("b") + "<w:r><w:cr/></w:r>" + &run("c")));
    assert_eq!(md(&body), "a  \nb  \nc");
    assert_eq!(visible(&md(&body)), "a\nb\nc");
}
