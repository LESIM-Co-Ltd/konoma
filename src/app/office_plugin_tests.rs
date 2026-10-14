//! The Word and presentation plugins on the host's plugin road: the representations the plugin
//! offers (`R` cycles through them), the anchored pages (`J`/`K`, the status chip) and how the
//! tab's choice reaches the plugin. The conversion and the drawing are tested in
//! `crates/konoma-office`; the keys over real files in `e2e_tests/{word,slides,deck_view}.rs`.

use super::*;
use crate::preview::office::docx::pptx::SlideInfo;
use crate::preview::office::docx::Document;
use crate::preview::office::slide_draw::{Fill, Rgba, SlideScene};
use crate::test_support::unique_tmp;

fn scene() -> SlideScene {
    SlideScene {
        width: 9_144_000.0,
        height: 5_143_500.0,
        background: Fill::Solid(Rgba::WHITE),
        underlay: Vec::new(),
        items: Vec::new(),
        truncated: false,
    }
}

/// A deck of `n` slides, each 20 lines of text, with pictures when `pictures`.
fn deck(n: usize, pictures: bool) -> Document {
    let mut md = String::new();
    let mut picture_md = String::new();
    for k in 0..n {
        md.push_str(&format!("## Slide {}: T{}\n", k + 1, k + 1));
        picture_md.push_str(&format!("## Slide {}: T{}\n\n![s](x)\n\n", k + 1, k + 1));
        for j in 1..20 {
            md.push_str(&format!("body {k}.{j}\n"));
        }
    }
    Document {
        markdown: md,
        slides: (1..=n)
            .map(|number| SlideInfo {
                number,
                title: format!("T{number}"),
                hidden: false,
            })
            .collect(),
        slide_scenes: if pictures {
            (0..n).map(|_| scene()).collect()
        } else {
            Vec::new()
        },
        slide_keys: if pictures {
            (1..=n)
                .map(|k| format!("office-img://bbbbbbbbbbbb/slide-{k}.svg"))
                .collect()
        } else {
            Vec::new()
        },
        picture_markdown: if pictures { picture_md } else { String::new() },
        ..Document::default()
    }
}

/// An `App` with a deck on screen (the text representation: no terminal picker).
fn app_with_deck(n: usize, pictures: bool) -> (App, crate::test_support::TmpDir) {
    let dir = unique_tmp("konoma_office_plugin");
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = App::new(dir.to_path_buf(), Config::default()).unwrap();
    app.show_document_for_test(dir.join("d.pptx"), deck(n, pictures));
    app.tab.preview_viewport = 30;
    app.ensure_md_cache(60);
    (app, dir)
}

#[test]
fn the_tabs_choice_of_representation_reaches_every_request() {
    let (mut app, _dir) = app_with_deck(3, true);
    assert_eq!(app.plugin_view_req().repr, None, "no choice: the default");
    app.tab.repr = Some(konoma_office::REPR_TEXT.to_string());
    assert_eq!(
        app.plugin_view_req().repr.as_deref(),
        Some(konoma_office::REPR_TEXT)
    );
}

#[test]
fn the_anchored_pages_are_the_pages_the_page_keys_and_the_indicator_turn() {
    let (mut app, _dir) = app_with_deck(3, false);
    assert!(app.is_deck());
    assert_eq!(app.page_indicator(), Some((1, 3)));
    assert!(app.can_turn_pages());
    app.page_turn(1);
    assert_eq!(
        app.page_indicator(),
        Some((2, 3)),
        "J goes to the next page"
    );
    app.page_turn(1);
    app.page_turn(1);
    assert_eq!(app.page_indicator(), Some((3, 3)), "and stops at the last");
    app.page_turn(-1);
    assert_eq!(app.page_indicator(), Some((2, 3)));
}

#[test]
fn a_single_page_document_and_a_word_document_have_nothing_to_turn() {
    let (app, _dir) = app_with_deck(1, false);
    assert_eq!(app.page_indicator(), Some((1, 1)));
    assert!(!app.can_turn_pages());

    let dir = unique_tmp("konoma_office_plugin_word");
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = App::new(dir.to_path_buf(), Config::default()).unwrap();
    app.show_document_for_test(
        dir.join("w.docx"),
        Document {
            markdown: "## A\n\ntext\n\n## B\n\ntext\n".into(),
            ..Document::default()
        },
    );
    app.ensure_md_cache(60);
    assert!(app.is_document() && !app.is_deck());
    assert_eq!(app.page_indicator(), None);
    assert!(!app.can_turn_pages());
}

#[test]
fn r_asks_the_plugin_for_the_other_representation_and_the_view_follows_the_answer() {
    let (mut app, _dir) = app_with_deck(3, true);
    // No picker: the plugin's default for this terminal is the text.
    assert!(app.is_deck() && !app.deck_picture_view());
    assert_eq!(
        app.deck_view_hint(),
        Some(crate::i18n::Msg::HintDeckSlides),
        "R offers the other one"
    );
    app.toggle_deck_view();
    assert!(app.deck_picture_view(), "the answer landed on the spot");
    assert_eq!(
        app.tab.repr.as_deref(),
        Some(konoma_office::REPR_PICTURE),
        "and is the tab's choice"
    );
    assert_eq!(app.deck_view_hint(), Some(crate::i18n::Msg::HintDeckText));
    assert!(app.deck_slide_url_for_test(1).is_some());
    app.toggle_deck_view();
    assert!(!app.deck_picture_view());
    assert_eq!(app.tab.repr.as_deref(), Some(konoma_office::REPR_TEXT));
    assert!(
        app.deck_slide_url_for_test(1).is_none(),
        "no slide pictures in text"
    );
}

#[test]
fn r_is_ignored_and_the_choice_stays_while_a_job_holds_the_session() {
    let (mut app, _dir) = app_with_deck(3, true);
    let held = app.plugin.as_mut().unwrap().session.take();
    assert!(held.is_some());
    app.toggle_deck_view();
    assert_eq!(app.tab.repr, None, "the choice was not recorded");
    assert!(!app.deck_picture_view());
    // The job gives the session back: R works again.
    app.plugin.as_mut().unwrap().session = held;
    app.toggle_deck_view();
    assert!(app.deck_picture_view());
}

#[test]
fn a_deck_without_pictures_offers_no_r() {
    let (mut app, _dir) = app_with_deck(3, false);
    assert_eq!(app.deck_view_hint(), None);
    assert_eq!(app.deck_view_help(), None);
    app.toggle_deck_view();
    assert_eq!(app.tab.repr, None, "nothing to switch to");
    assert!(!app.deck_picture_view());
}

#[test]
fn the_slide_in_view_is_kept_across_a_switch_of_representation() {
    let (mut app, _dir) = app_with_deck(5, true);
    app.page_turn(1);
    app.page_turn(1);
    assert_eq!(app.anchored_position(), Some((3, 5)));
    app.toggle_deck_view();
    // The new representation is laid out by the next draw, which scrolls to the slide kept.
    app.ensure_md_cache(60);
    app.apply_pending_slide();
    assert_eq!(app.anchored_position(), Some((3, 5)), "still slide 3");
}

#[test]
fn leaving_the_document_frees_both_the_pictures_and_the_plugins_session() {
    let (mut app, dir) = app_with_deck(2, true);
    app.toggle_deck_view();
    assert!(app.document_ready() && app.plugin.is_some());
    app.set_preview_kind(Some(PreviewKind::Text(dir.join("a.txt"))));
    assert!(app.document.is_none(), "the converted document goes");
    assert!(app.plugin.is_none(), "and so does the plugin's show");
}
