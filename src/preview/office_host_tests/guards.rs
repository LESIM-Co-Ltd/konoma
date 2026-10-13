//! Watchmen for the two places where the Office core repeats something the host owns: it does
//! not know the host's table row cap or the host's translations, so it carries its own copy of the
//! one number and the three English words, and the core's tests spell the Japanese ones out. If
//! the host changes either, one of these fails.

use crate::i18n::{slide_labels, Lang};
use konoma_office_core::container::{Limits, DEFAULT_MAX_ROWS};
use konoma_office_core::docx::{DocOptions, SlideLabels};
use konoma_office_core::test_labels::Lang as CoreLang;

#[test]
fn the_cores_default_row_cap_is_the_table_previews() {
    assert_eq!(DEFAULT_MAX_ROWS, crate::preview::table::MAX_ROWS);
    assert_eq!(Limits::default().max_rows, crate::preview::table::MAX_ROWS);
}

#[test]
fn the_slide_words_the_host_hands_in_are_the_ones_the_cores_tests_spell_out() {
    assert_eq!(slide_labels(Lang::En), CoreLang::En.labels());
    assert_eq!(slide_labels(Lang::Jp), CoreLang::Jp.labels());
}

#[test]
fn the_cores_default_slide_words_are_the_english_ones() {
    assert_eq!(SlideLabels::default(), slide_labels(Lang::En));
    assert_eq!(DocOptions::default().labels, slide_labels(Lang::En));
}
