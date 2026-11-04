// Expected contexts are literals from 7359773, never translations read from the live catalog.
pub fn assert_english_context(label: &str, text: &str, expected: &str) {
    assert!(
        !text.chars().any(|c| ('\u{3000}'..='\u{9fff}').contains(&c)),
        "Japanese in English {label}: {text}"
    );
    assert!(text.starts_with(expected), "changed English {label}: expected {expected:?}, got {text:?}");
    let suffix = &text[expected.len()..];
    assert!(suffix.is_empty() || suffix.starts_with(": ") || suffix.starts_with('\n'), "changed English context in {label}: {text}");
}
