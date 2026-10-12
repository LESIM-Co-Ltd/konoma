//! Test-only: the words of a converted presentation in English and Japanese, spelled out here
//! because the core owns no translations (the host passes its own in; a host test keeps the two
//! spellings equal).
// Compiled outside `cfg(test)` only for the host's tests (feature `test-hooks`), where the
// `#[test]` functions are gone and what only they used is unused.
#![cfg_attr(
    not(test),
    allow(unused_imports, dead_code, unused_variables, unused_mut)
)]

use crate::docx::SlideLabels;

/// The two languages the tests exercise.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lang {
    En,
    Jp,
}

impl Lang {
    /// The labels of this language.
    pub fn labels(self) -> SlideLabels {
        match self {
            Lang::En => SlideLabels {
                heading: "Slide".to_string(),
                hidden_suffix: " (hidden)".to_string(),
                notes: "Notes".to_string(),
            },
            Lang::Jp => SlideLabels {
                heading: "スライド".to_string(),
                hidden_suffix: " (非表示)".to_string(),
                notes: "ノート".to_string(),
            },
        }
    }
}
