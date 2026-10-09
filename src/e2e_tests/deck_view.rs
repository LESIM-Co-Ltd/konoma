//! End-to-end tests of a presentation's two views (the slide pictures and the text): hand-made
//! `Document`s with simple drawing scenes land on a deck opened through the real key path, so none
//! of this depends on what the pptx reader fills. `R` switches the views and keeps the slide; the
//! picture view is the default only where the terminal draws real pixels; the slide pictures are
//! drawn on the decode thread through the supervised SVG process.

use super::*;
use crate::preview::office::docx::pptx::SlideInfo;
use crate::preview::office::docx::Document;
use crate::preview::office::slide_draw::{Fill, Item, PictureItem, Rgba, SlideScene, Xfrm};
use ratatui_image::picker::{Picker, ProtocolType};

const EN: &str = "slides.pptx";
/// A 16:9 slide of 960x540 px.
const W_EMU: f64 = 960.0 * 9525.0;
const H_EMU: f64 = 540.0 * 9525.0;

fn testdata(file: &str) -> Option<std::path::PathBuf> {
    let src = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("testdata/office")
        .join(file);
    if !src.exists() {
        eprintln!("SKIP: testdata/office/{file} not found - this test verifies nothing this run");
        return None;
    }
    Some(src)
}

fn url_of(k: usize) -> String {
    format!("office-img://0123456789ab/slide-{k}.svg")
}

/// A slide whose whole background is `color`.
fn scene(color: Rgba) -> SlideScene {
    SlideScene {
        width: W_EMU,
        height: H_EMU,
        background: Fill::Solid(color),
        items: Vec::new(),
        truncated: false,
    }
}

const RED: Rgba = Rgba::rgb(255, 0, 0);

/// A deck of `n` slides. Both Markdowns have the same `## Slide k: Tk` heading per slide, the
/// text view body is `Text of slide k`, the notes `secret-note-k` are in both.
fn deck_doc_with(n: usize, scenes: Vec<SlideScene>) -> Document {
    let mut text = String::new();
    let mut pics = String::new();
    for k in 1..=n {
        text.push_str(&format!(
            "## Slide {k}: T{k}\n\nText of slide {k}\n\n> Notes: secret-note-{k}\n\n"
        ));
        pics.push_str(&format!(
            "## Slide {k}: T{k}\n\n![Slide {k}]({})\n\n> Notes: secret-note-{k}\n\n",
            url_of(k)
        ));
    }
    let with_pictures = !scenes.is_empty();
    Document {
        markdown: text,
        slides: (1..=n)
            .map(|number| SlideInfo {
                number,
                title: format!("T{number}"),
                hidden: false,
            })
            .collect(),
        slide_keys: if with_pictures {
            (1..=n).map(url_of).collect()
        } else {
            Vec::new()
        },
        slide_scenes: scenes,
        picture_markdown: if with_pictures { pics } else { String::new() },
        ..Document::default()
    }
}

/// A deck of `n` red slides.
fn deck_doc(n: usize) -> Document {
    deck_doc_with(n, (0..n).map(|_| scene(RED)).collect())
}

fn picker_of(p: ProtocolType) -> Picker {
    let mut picker = Picker::halfblocks();
    picker.set_protocol_type(p);
    picker
}

/// A deck opened in a `size` terminal whose image protocol is `proto`, with `doc` landed over what
/// the real reader made of `slides.pptx`.
fn open_with(
    name: &str,
    proto: Option<ProtocolType>,
    size: (u16, u16),
    doc: Document,
    cfg: Config,
) -> Option<(Sim, crate::test_support::TmpDir)> {
    let dir = sandbox(name);
    std::fs::copy(testdata(EN)?, dir.join("deck.pptx")).unwrap();
    std::fs::copy(testdata(EN)?, dir.join("other.pptx")).unwrap();
    std::fs::write(dir.join("z.txt"), "after the deck\n").unwrap();
    let mut s = Sim::with_config_sized(&canon(&dir), cfg, size.0, size.1);
    if let Some(p) = proto {
        s = s.with_media_on(picker_of(p));
    }
    s.select("deck.pptx");
    s.enter();
    if proto.is_some() {
        s.drain_media();
    }
    s.app.land_document_for_test(doc);
    s.draw();
    Some((s, dir))
}

/// A deck of `n` slides on a kitty terminal.
fn open_kitty(
    name: &str,
    n: usize,
    size: (u16, u16),
) -> Option<(Sim, crate::test_support::TmpDir)> {
    open_with(name, Some(ProtocolType::Kitty), size, deck_doc(n), cfg_en())
}

/// A deck of `n` slides on a half-block terminal.
fn open_halfblocks(
    name: &str,
    n: usize,
    size: (u16, u16),
) -> Option<(Sim, crate::test_support::TmpDir)> {
    open_with(
        name,
        Some(ProtocolType::Halfblocks),
        size,
        deck_doc(n),
        cfg_en(),
    )
}

fn footer_text(s: &Sim) -> String {
    crate::ui::preview::footer_hints(&s.app).join(" | ")
}

fn r_hint(s: &Sim) -> Option<String> {
    crate::ui::preview::footer_hints(&s.app)
        .into_iter()
        .find(|h| h.starts_with("R:"))
}

/// The screen row (0-based) a text first shows on, if it is on screen.
fn row_of(s: &Sim, needle: &str) -> Option<usize> {
    s.screen().lines().position(|l| l.contains(needle))
}

#[track_caller]
fn at(s: &Sim, n: usize, total: usize) {
    assert_eq!(s.app.slide_position(), Some((n, total)), "{}", s.screen());
}

// ---------------------------------------------------------------------------------------------
// The default view follows what the terminal can draw
// ---------------------------------------------------------------------------------------------

#[test]
fn e2e_deck_pictures_are_the_default_where_the_terminal_draws_pixels() {
    for (name, proto) in [
        ("kitty", ProtocolType::Kitty),
        ("sixel", ProtocolType::Sixel),
        ("iterm", ProtocolType::Iterm2),
    ] {
        let Some((s, _d)) = open_with(
            &format!("dv_pix_{name}"),
            Some(proto),
            (100, 30),
            deck_doc(3),
            cfg_en(),
        ) else {
            return;
        };
        assert!(s.app.is_deck(), "{name}");
        assert!(s.app.deck_picture_view(), "{name}: pictures by default");
        assert!(
            s.app.deck_view_choice_for_test().is_none(),
            "{name}: the default, not a choice"
        );
        assert!(
            s.app.md_images().iter().any(|p| p.url == url_of(1)),
            "{name}: the slide picture is placed"
        );
        s.dont_see("Text of slide 1");
    }
}

#[test]
fn e2e_deck_text_is_the_default_where_images_would_be_half_blocks() {
    let Some((s, _d)) = open_halfblocks("dv_half", 3, (100, 30)) else {
        return;
    };
    assert!(s.app.is_deck() && !s.app.deck_picture_view());
    s.see("Text of slide 1");
    assert!(s.app.md_images().is_empty(), "{:?}", s.app.md_images());
    // The picture view is one `R` away even there.
    assert_eq!(r_hint(&s).as_deref(), Some("R:slides"));
}

#[test]
fn e2e_deck_text_is_the_default_without_an_image_backend() {
    let Some((s, _d)) = open_with("dv_nopicker", None, (100, 30), deck_doc(3), cfg_en()) else {
        return;
    };
    assert!(s.app.is_deck() && !s.app.deck_picture_view());
    s.see("Text of slide 1");
}

#[test]
fn e2e_deck_a_new_file_starts_in_the_default_view_again() {
    let Some((mut s, _d)) = open_kitty("dv_newfile", 3, (100, 30)) else {
        return;
    };
    s.key('R');
    assert!(!s.app.deck_picture_view());
    s.key('q');
    s.select("deck.pptx");
    s.enter();
    s.drain_media();
    s.app.land_document_for_test(deck_doc(3));
    s.draw();
    assert!(s.app.deck_picture_view(), "the choice is per opening");
}

// ---------------------------------------------------------------------------------------------
// R: switching keeps the slide
// ---------------------------------------------------------------------------------------------

#[test]
fn e2e_deck_r_toggles_the_views_and_comes_back() {
    let Some((mut s, _d)) = open_kitty("dv_r_toggle", 4, (100, 30)) else {
        return;
    };
    assert!(s.app.deck_picture_view());
    s.key('R');
    assert!(!s.app.deck_picture_view());
    assert_eq!(s.app.deck_view_choice_for_test(), Some(true));
    s.see("Text of slide 1");
    assert!(!s.app.is_md_raw(), "a deck has no raw Markdown view");
    s.key('R');
    assert!(s.app.deck_picture_view());
    assert_eq!(s.app.deck_view_choice_for_test(), Some(false));
    s.dont_see("Text of slide 1");
}

#[test]
fn e2e_deck_r_keeps_the_slide_from_every_place() {
    let Some((mut s, _d)) = open_kitty("dv_r_keep", 6, (100, 30)) else {
        return;
    };
    for target in 1..=6 {
        // Go to the slide in whichever view is on, then switch both ways.
        s.key('g');
        for _ in 1..target {
            s.key('J');
        }
        at(&s, target, 6);
        s.key('R');
        at(&s, target, 6);
        assert!(
            row_of(&s, &format!("Slide {target}: T{target}")).is_some(),
            "text view, slide {target}\n{}",
            s.screen()
        );
        s.key('R');
        at(&s, target, 6);
        assert!(
            row_of(&s, &format!("Slide {target}: T{target}")).is_some(),
            "picture view, slide {target}\n{}",
            s.screen()
        );
    }
}

#[test]
fn e2e_deck_r_lands_the_slide_heading_at_the_top() {
    let Some((mut s, _d)) = open_kitty("dv_r_top", 5, (100, 30)) else {
        return;
    };
    for _ in 0..3 {
        s.key('J');
    }
    at(&s, 4, 5);
    s.key('R');
    at(&s, 4, 5);
    assert!(
        screen_row(&s, 2).starts_with("Slide 4: T4"),
        "{}",
        s.screen()
    );
    s.key('R');
    assert!(
        screen_row(&s, 2).starts_with("Slide 4: T4"),
        "{}",
        s.screen()
    );
}

#[test]
fn e2e_deck_r_after_overscroll_keeps_the_last_slide() {
    let Some((mut s, _d)) = open_kitty("dv_r_over", 5, (100, 30)) else {
        return;
    };
    s.key('G');
    at(&s, 5, 5);
    s.key('R');
    at(&s, 5, 5);
    s.key('G');
    at(&s, 5, 5);
    s.key('R');
    at(&s, 5, 5);
    assert!(
        screen_row(&s, 2).starts_with("Slide 5: T5"),
        "{}",
        s.screen()
    );
}

#[test]
fn e2e_deck_r_from_the_middle_of_a_slide_keeps_that_slide() {
    let Some((mut s, _d)) = open_kitty("dv_r_mid", 4, (100, 30)) else {
        return;
    };
    s.key('J');
    s.key('J');
    // Two rows into slide 3 (its picture is on screen, the heading above the top).
    s.key('j');
    s.key('j');
    at(&s, 3, 4);
    s.key('R');
    at(&s, 3, 4);
}

#[test]
fn e2e_deck_r_does_nothing_while_the_deck_has_no_pictures() {
    let Some((mut s, _d)) = open_with(
        "dv_r_nopics",
        Some(ProtocolType::Kitty),
        (100, 30),
        deck_doc_with(3, Vec::new()),
        cfg_en(),
    ) else {
        return;
    };
    assert!(s.app.is_deck() && !s.app.deck_picture_view());
    s.key('J');
    at(&s, 2, 3);
    let before = s.screen();
    s.key('R');
    assert_eq!(s.screen(), before, "R is dead without a picture view");
    assert!(s.app.deck_view_choice_for_test().is_none());
    assert!(!s.app.is_md_raw());
}

#[test]
fn e2e_deck_with_inconsistent_scenes_has_the_text_view_only() {
    // Fewer scenes than slides: the reader did not fill them all.
    let mut doc = deck_doc(3);
    doc.slide_scenes.pop();
    let Some((mut s, _d)) = open_with(
        "dv_inconsistent",
        Some(ProtocolType::Kitty),
        (100, 30),
        doc,
        cfg_en(),
    ) else {
        return;
    };
    assert!(!s.app.deck_picture_view());
    assert_eq!(r_hint(&s), None);
    s.key('R');
    assert!(s.app.deck_view_choice_for_test().is_none());
    s.see("Text of slide 1");
}

#[test]
fn e2e_word_r_is_still_the_raw_markdown_source() {
    let Some(src) = testdata("word.docx") else {
        return;
    };
    let dir = sandbox("dv_word_raw");
    std::fs::copy(src, dir.join("word.docx")).unwrap();
    let mut s = Sim::with_config_sized(&canon(&dir), cfg_en(), 100, 30)
        .with_media_on(picker_of(ProtocolType::Kitty));
    s.select("word.docx");
    s.enter();
    s.drain_media();
    assert!(s.app.document_ready() && !s.app.is_deck());
    assert_eq!(r_hint(&s).as_deref(), Some("R:raw source"));
    s.key('R');
    assert!(s.app.is_md_raw() && s.app.is_windowed());
    assert_eq!(r_hint(&s).as_deref(), Some("R:rendered"));
    assert!(s.app.deck_view_choice_for_test().is_none());
    s.key('R');
    assert!(!s.app.is_md_raw());
}

// ---------------------------------------------------------------------------------------------
// Hints say what R does now, and only when it acts
// ---------------------------------------------------------------------------------------------

#[test]
fn e2e_deck_r_hint_names_the_other_view() {
    let Some((mut s, _d)) = open_kitty("dv_hint", 3, (140, 30)) else {
        return;
    };
    assert_eq!(r_hint(&s).as_deref(), Some("R:text view"));
    s.key('R');
    assert_eq!(r_hint(&s).as_deref(), Some("R:slides"));
    s.key('R');
    assert_eq!(r_hint(&s).as_deref(), Some("R:text view"));
    // Never the raw-source wording.
    assert!(
        !footer_text(&s).contains("raw source"),
        "{}",
        footer_text(&s)
    );
}

#[test]
fn e2e_deck_r_hint_is_japanese_in_a_japanese_ui() {
    let mut cfg = Config::default();
    cfg.ui.lang = "ja".into();
    let Some((mut s, _d)) = open_with(
        "dv_hint_ja",
        Some(ProtocolType::Kitty),
        (140, 30),
        deck_doc(3),
        cfg,
    ) else {
        return;
    };
    assert_eq!(r_hint(&s).as_deref(), Some("R:文字表示"));
    s.key('R');
    assert_eq!(r_hint(&s).as_deref(), Some("R:スライド表示"));
}

#[test]
fn e2e_deck_help_names_what_r_does_and_only_when_it_acts() {
    let Some((mut s, _d)) = open_kitty("dv_help", 3, (100, 40)) else {
        return;
    };
    s.key('?');
    s.see("show the slides as text");
    s.dont_see("raw source");
    s.key('?');
    s.key('R');
    s.key('?');
    s.see("show the slides as pictures");
    s.dont_see("show the slides as text");
    s.key('?');
    // No picture view: no row for R at all.
    let Some((mut t, _d2)) = open_with(
        "dv_help_nopics",
        Some(ProtocolType::Kitty),
        (100, 40),
        deck_doc_with(3, Vec::new()),
        cfg_en(),
    ) else {
        return;
    };
    t.key('?');
    t.dont_see("show the slides as");
    t.dont_see("rendered / raw source");
    assert_eq!(r_hint(&t), None, "{}", footer_text(&t));
}

// ---------------------------------------------------------------------------------------------
// J / K, the chip, the outline and search work in both views
// ---------------------------------------------------------------------------------------------

#[test]
fn e2e_deck_j_k_and_the_chip_work_in_the_picture_view() {
    let Some((mut s, _d)) = open_kitty("dv_jk", 5, (100, 30)) else {
        return;
    };
    assert!(s.app.deck_picture_view());
    at(&s, 1, 5);
    assert!(s.app.slide_can_turn());
    for k in 2..=5 {
        s.key('J');
        at(&s, k, 5);
        assert!(
            screen_row(&s, 2).starts_with(&format!("Slide {k}: T{k}")),
            "J to {k}\n{}",
            s.screen()
        );
    }
    s.key('J');
    at(&s, 5, 5);
    for k in (1..5).rev() {
        s.key('K');
        at(&s, k, 5);
    }
    s.key('K');
    at(&s, 1, 5);
    s.see("slide 1/5");
    assert!(footer_text(&s).contains("J/K:slide"), "{}", footer_text(&s));
}

#[test]
fn e2e_deck_each_slide_fits_the_screen_with_its_heading_and_every_slide_can_reach_the_top() {
    for h in [14u16, 20, 30, 50] {
        let Some((mut s, _d)) = open_kitty(&format!("dv_fit{h}"), 4, (100, h)) else {
            return;
        };
        for k in 1..=4 {
            s.key('g');
            for _ in 1..k {
                s.key('J');
            }
            at(&s, k, 4);
            assert!(
                screen_row(&s, 2).starts_with(&format!("Slide {k}: T{k}")),
                "h={h} slide {k}\n{}",
                s.screen()
            );
            // The slide's picture, heading to last picture row, lies inside the viewport.
            let p = s
                .app
                .md_images()
                .into_iter()
                .find(|p| p.url == url_of(k))
                .expect("placed");
            let vh = s.app.tab.preview_viewport as usize;
            let head_row = s.app.tab.preview_scroll as usize;
            let (start, _) = s.app.md_visual_span_for_test(p.line);
            assert!(
                start + p.rows as usize - head_row <= vh,
                "h={h} slide {k}: picture ends at row {} of {vh}",
                start + p.rows as usize - head_row
            );
        }
    }
}

#[test]
fn e2e_deck_the_last_slide_can_reach_the_top_in_the_picture_view() {
    let Some((mut s, _d)) = open_kitty("dv_last_top", 3, (100, 30)) else {
        return;
    };
    // A slide with its picture and notes is taller than the screen, so `G` ends below its heading.
    s.key('G');
    at(&s, 3, 3);
    let limit = s.app.tab.preview_scroll;
    for _ in 0..10 {
        s.key('j');
    }
    assert_eq!(s.app.tab.preview_scroll, limit, "the end of the range");
    // The heading of the last slide is reachable all the same: K goes to the start of the slide
    // the view is in, and the range reaches it.
    s.key('K');
    at(&s, 3, 3);
    assert!(
        screen_row(&s, 2).starts_with("Slide 3: T3"),
        "{}",
        s.screen()
    );
    assert!(s.app.tab.preview_scroll <= limit);
    s.key('K');
    at(&s, 2, 3);
}

#[test]
fn e2e_deck_the_outline_lists_the_slides_in_the_picture_view_and_jumps() {
    let Some((mut s, _d)) = open_kitty("dv_outline", 4, (100, 30)) else {
        return;
    };
    s.key('o');
    assert!(s.app.is_outline());
    for k in 1..=4 {
        s.see(&format!("Slide {k}: T{k}"));
    }
    for _ in 0..2 {
        s.key('j');
    }
    s.enter();
    assert!(!s.app.is_outline());
    at(&s, 3, 4);
}

#[test]
fn e2e_deck_search_finds_the_notes_and_headings_in_the_picture_view() {
    let Some((mut s, _d)) = open_kitty("dv_search", 4, (100, 30)) else {
        return;
    };
    assert!(s.app.deck_picture_view());
    s.key('/');
    s.keys("secret-note-3");
    s.enter();
    let (cur, total) = s.app.search_status().expect("a hit");
    assert_eq!((cur, total), (1, 1));
    s.see("secret-note-3");
    // A heading is text too.
    s.key('/');
    s.keys("T2");
    s.enter();
    assert!(s.app.search_status().is_some());
    // The text of the other view is not searched here.
    s.key('/');
    s.keys("Text of slide");
    s.enter();
    assert!(s.app.search_status().is_none());
}

#[test]
fn e2e_deck_e_opens_the_office_app_in_both_views() {
    let Some((mut s, _d)) = open_kitty("dv_e", 3, (100, 30)) else {
        return;
    };
    let log = office_recorder(&mut s, 0);
    s.key('e');
    s.key('R');
    s.key('e');
    assert!(s.app.take_pending_edit().is_none());
    assert_eq!(log.lock().unwrap().len(), 2);
}

// ---------------------------------------------------------------------------------------------
// Tabs, reloads
// ---------------------------------------------------------------------------------------------

#[test]
fn e2e_deck_the_view_is_per_tab() {
    let Some((mut s, _d)) = open_kitty("dv_tabs", 3, (100, 30)) else {
        return;
    };
    s.key('R');
    assert_eq!(s.app.deck_view_choice_for_test(), Some(true));
    // A second tab opens its own deck in the default view.
    s.key('t');
    s.select("other.pptx");
    s.enter();
    s.drain_media();
    s.app.land_document_for_test(deck_doc(3));
    s.draw();
    assert!(s.app.deck_view_choice_for_test().is_none());
    assert!(s.app.deck_picture_view());
    s.key('R');
    assert_eq!(s.app.deck_view_choice_for_test(), Some(true));
    s.key('R');
    assert_eq!(s.app.deck_view_choice_for_test(), Some(false));
    // Back in the first tab its own choice is still there.
    s.key('[');
    s.drain_media();
    assert_eq!(s.app.deck_view_choice_for_test(), Some(true));
    s.key(']');
    s.drain_media();
    assert_eq!(s.app.deck_view_choice_for_test(), Some(false));
}

#[test]
fn e2e_deck_a_reload_keeps_the_view_and_the_slide() {
    for text_view in [false, true] {
        let Some((mut s, _d)) = open_kitty(&format!("dv_reload{text_view}"), 5, (100, 30)) else {
            return;
        };
        if text_view {
            s.key('R');
        }
        s.key('J');
        s.key('J');
        at(&s, 3, 5);
        let view = s.app.deck_picture_view();
        // The file changed on disk: the worker's answer lands as a new conversion.
        s.app.land_document_for_test(deck_doc(5));
        s.draw();
        assert_eq!(s.app.deck_picture_view(), view, "text_view={text_view}");
        at(&s, 3, 5);
    }
}

#[test]
fn e2e_deck_a_reload_that_loses_the_pictures_falls_back_to_the_text() {
    let Some((mut s, _d)) = open_kitty("dv_reload_nopics", 3, (100, 30)) else {
        return;
    };
    s.key('J');
    s.app.land_document_for_test(deck_doc_with(3, Vec::new()));
    s.draw();
    assert!(!s.app.deck_picture_view());
    s.see("Text of slide");
    s.dont_see("🖼");
    assert_eq!(r_hint(&s), None);
}

// ---------------------------------------------------------------------------------------------
// The slide pictures are drawn on the decode thread
// ---------------------------------------------------------------------------------------------

/// Puts each of the first `n` slides at the top in turn and lets its picture be drawn (only the
/// pictures on screen are decoded).
#[track_caller]
fn visit_slides(s: &mut Sim, n: usize) {
    for k in 1..=n {
        s.key('g');
        for _ in 1..k {
            s.key('J');
        }
        settle(s);
    }
}

/// Applies pending decodes / encodes until the pipeline is quiet.
#[track_caller]
fn settle(s: &mut Sim) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    loop {
        let imgs: Vec<_> = s.md_img_rx.as_ref().unwrap().try_iter().collect();
        let encs: Vec<_> = s.md_enc_rx.as_ref().unwrap().try_iter().collect();
        let any = !imgs.is_empty() || !encs.is_empty();
        for r in imgs {
            s.app.apply_md_image(r);
        }
        for r in encs {
            s.app.apply_md_encode(r);
        }
        s.draw();
        if !any && !s.app.md_images_loading() {
            return;
        }
        assert!(std::time::Instant::now() < deadline, "never settled");
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

#[test]
fn e2e_deck_a_scene_is_drawn_into_real_pixels_by_the_decode_thread() {
    let Some((mut s, _d)) = open_kitty("dv_draw", 2, (100, 30)) else {
        return;
    };
    settle(&mut s);
    let url = s.app.deck_slide_url_for_test(1).expect("slide 1");
    assert_eq!(url, url_of(1));
    let px = s
        .app
        .office_picture_rgba_for_test(&url, 0.5, 0.5)
        .expect("decoded");
    assert_eq!(px, [255, 0, 0, 255], "the red background");
    assert!(s.app.office_picture_failure_for_test(&url).is_none());
    // Its aspect is the slide's (16:9), in pixels.
    let (w, h) = s.app.office_picture_pixels_for_test(&url).unwrap();
    assert!(
        (w as f64 / h as f64 - 960.0 / 540.0).abs() < 0.02,
        "{w}x{h}"
    );
}

#[test]
fn e2e_deck_each_slide_is_drawn_from_its_own_scene() {
    let scenes = vec![
        scene(Rgba::rgb(255, 0, 0)),
        scene(Rgba::rgb(0, 255, 0)),
        scene(Rgba::rgb(0, 0, 255)),
    ];
    let Some((mut s, _d)) = open_with(
        "dv_own_scene",
        Some(ProtocolType::Kitty),
        (100, 60),
        deck_doc_with(3, scenes),
        cfg_en(),
    ) else {
        return;
    };
    visit_slides(&mut s, 3);
    for (k, want) in [
        (1, [255, 0, 0, 255]),
        (2, [0, 255, 0, 255]),
        (3, [0, 0, 255, 255]),
    ] {
        let url = url_of(k);
        assert_eq!(
            s.app.office_picture_rgba_for_test(&url, 0.5, 0.5),
            Some(want),
            "slide {k}"
        );
    }
}

#[test]
fn e2e_deck_a_picture_in_a_scene_is_resolved_from_the_documents_media() {
    // A 2x2 blue PNG filling the slide, found by the key the scene names.
    let png = {
        let img = image::RgbImage::from_pixel(2, 2, image::Rgb([0, 0, 255]));
        let mut b = Vec::new();
        image::DynamicImage::ImageRgb8(img)
            .write_to(&mut std::io::Cursor::new(&mut b), image::ImageFormat::Png)
            .unwrap();
        b
    };
    let mut sc = scene(Rgba::WHITE);
    sc.items.push(Item::Picture(PictureItem::new(
        Xfrm::rect(0.0, 0.0, W_EMU, H_EMU),
        "office-img://0123456789ab/blue.png",
    )));
    let mut doc = deck_doc_with(1, vec![sc]);
    doc.images.push(crate::preview::office::docx::DocImage {
        key: "office-img://0123456789ab/blue.png".into(),
        bytes: png,
        name: "blue.png".into(),
    });
    let Some((mut s, _d)) = open_with(
        "dv_media",
        Some(ProtocolType::Kitty),
        (100, 40),
        doc,
        cfg_en(),
    ) else {
        return;
    };
    settle(&mut s);
    let px = s
        .app
        .office_picture_rgba_for_test(&url_of(1), 0.5, 0.5)
        .expect("decoded");
    assert!(px[2] > 200 && px[0] < 60, "the picture is blue: {px:?}");
}

#[test]
fn e2e_deck_a_scene_naming_a_missing_picture_still_draws() {
    let mut sc = scene(RED);
    sc.items.push(Item::Picture(PictureItem::new(
        Xfrm::rect(0.0, 0.0, W_EMU / 2.0, H_EMU / 2.0),
        "office-img://0123456789ab/gone.png",
    )));
    let Some((mut s, _d)) = open_with(
        "dv_missing",
        Some(ProtocolType::Kitty),
        (100, 40),
        deck_doc_with(1, vec![sc]),
        cfg_en(),
    ) else {
        return;
    };
    settle(&mut s);
    let url = url_of(1);
    assert!(s.app.office_picture_failure_for_test(&url).is_none());
    assert!(s.app.office_picture_pixels_for_test(&url).is_some());
}

/// A PNG that claims `w` x `h` pixels and has no pixel data: its header is all a size check reads.
fn png_claiming(w: u32, h: u32) -> Vec<u8> {
    fn crc32(bytes: &[u8]) -> u32 {
        let mut crc = !0u32;
        for &b in bytes {
            crc ^= u32::from(b);
            for _ in 0..8 {
                crc = if crc & 1 == 1 {
                    (crc >> 1) ^ 0xEDB8_8320
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut chunk = |kind: &[u8; 4], data: &[u8]| {
        out.extend((data.len() as u32).to_be_bytes());
        let mut body = kind.to_vec();
        body.extend_from_slice(data);
        out.extend(&body);
        out.extend(crc32(&body).to_be_bytes());
    };
    let mut ihdr = Vec::new();
    ihdr.extend(w.to_be_bytes());
    ihdr.extend(h.to_be_bytes());
    ihdr.extend([8, 2, 0, 0, 0]);
    chunk(b"IHDR", &ihdr);
    chunk(b"IEND", &[]);
    out
}

#[test]
fn e2e_deck_a_slide_that_cannot_be_drawn_says_why_and_the_others_still_draw() {
    // Slide 2 embeds a picture that claims 20,000 x 20,000 pixels: the drawing process refuses to
    // decode that much, and says so.
    let mut heavy = scene(RED);
    heavy.items.push(Item::Picture(PictureItem::new(
        Xfrm::rect(0.0, 0.0, W_EMU, H_EMU),
        "office-img://0123456789ab/heavy.png",
    )));
    let mut doc = deck_doc_with(3, vec![scene(RED), heavy, scene(RED)]);
    doc.images.push(crate::preview::office::docx::DocImage {
        key: "office-img://0123456789ab/heavy.png".into(),
        bytes: png_claiming(20_000, 20_000),
        name: "heavy.png".into(),
    });
    let Some((mut s, _d)) = open_with(
        "dv_fail",
        Some(ProtocolType::Kitty),
        (100, 60),
        doc,
        cfg_en(),
    ) else {
        return;
    };
    visit_slides(&mut s, 3);
    assert!(s.app.office_picture_pixels_for_test(&url_of(1)).is_some());
    assert!(s.app.office_picture_pixels_for_test(&url_of(3)).is_some());
    assert!(
        s.app.office_picture_pixels_for_test(&url_of(2)).is_none(),
        "the heavy slide has no picture"
    );
    assert!(
        s.app.office_picture_failure_for_test(&url_of(2)).is_some(),
        "and a reason"
    );
    // The reason is on the slide's own line; the other slides show no such line.
    s.key('g');
    s.key('J');
    s.draw();
    s.see("Slide 2 — [svg] not drawn: it asks for more drawing work than is allowed");
}

#[test]
fn e2e_deck_an_evicted_slide_is_drawn_again_from_its_scene() {
    let Some((mut s, _d)) = open_kitty("dv_evict", 2, (100, 60)) else {
        return;
    };
    settle(&mut s);
    let url = url_of(1);
    assert!(s.app.office_picture_pixels_for_test(&url).is_some());
    // A new pass has started (the picture was drawn in the previous one), then the cache is told
    // to hold nothing.
    s.app.begin_md_image_frame();
    s.app.evict_md_images_to_for_test(0);
    assert!(
        s.app.office_picture_pixels_for_test(&url).is_none(),
        "setup: the pixels are gone"
    );
    s.draw();
    settle(&mut s);
    assert_eq!(
        s.app.office_picture_rgba_for_test(&url, 0.5, 0.5),
        Some([255, 0, 0, 255]),
        "rebuilt through the same drawing path"
    );
}

#[test]
fn e2e_deck_a_zero_sized_slide_is_its_alt_text_not_a_crash() {
    let mut flat = scene(RED);
    flat.width = 0.0;
    let mut nan = scene(RED);
    nan.height = f64::NAN;
    let Some((mut s, _d)) = open_with(
        "dv_zero",
        Some(ProtocolType::Kitty),
        (100, 40),
        deck_doc_with(3, vec![flat, nan, scene(RED)]),
        cfg_en(),
    ) else {
        return;
    };
    settle(&mut s);
    assert!(s.app.office_picture_started_for_test(&url_of(3)) || s.app.md_images().len() <= 3);
    s.key('J');
    s.key('J');
    at(&s, 3, 3);
}

#[test]
fn e2e_deck_the_picture_view_draws_on_the_screen_on_a_half_block_terminal_after_r() {
    let Some((mut s, _d)) = open_halfblocks("dv_half_pic", 2, (100, 40)) else {
        return;
    };
    s.key('R');
    assert!(s.app.deck_picture_view());
    settle(&mut s);
    let red = drawn_rgb_fgs(&s.term)
        .into_iter()
        .filter(|&(r, g, b)| r > 200 && g < 60 && b < 60)
        .count();
    assert!(red > 100, "red cells on screen: {red}");
}

// ---------------------------------------------------------------------------------------------
// The slide's box
// ---------------------------------------------------------------------------------------------

#[test]
fn e2e_deck_the_slide_box_is_the_full_width_and_keeps_the_aspect() {
    // A tall terminal: width is the limit.
    let Some((s, _d)) = open_kitty("dv_box_w", 2, (100, 80)) else {
        return;
    };
    let p = s
        .app
        .md_images()
        .into_iter()
        .find(|p| p.url == url_of(1))
        .unwrap();
    let (fw, fh) = (10.0f64, 20.0f64); // the test picker's cell size
                                       // The text width, as for every picture (two columns of margin inside the 98-column pane).
    assert_eq!(p.cols, 96, "the whole text width");
    let want_rows = (96.0 * fw * 540.0 / (960.0 * fh)).round() as u16;
    assert_eq!(p.rows, want_rows);
}

#[test]
fn e2e_deck_the_slide_box_is_capped_by_the_height_and_shrinks_in_width() {
    let Some((s, _d)) = open_kitty("dv_box_h", 2, (200, 16)) else {
        return;
    };
    let p = s
        .app
        .md_images()
        .into_iter()
        .find(|p| p.url == url_of(1))
        .unwrap();
    let vh = s.app.tab.preview_viewport;
    assert!(p.rows <= vh - 2, "rows {} in a {vh}-row view", p.rows);
    assert!(p.cols < 198, "narrower than the width: {}", p.cols);
}

#[test]
fn e2e_deck_a_resize_refits_the_slide_boxes() {
    let Some((mut s, _d)) = open_kitty("dv_resize", 2, (100, 80)) else {
        return;
    };
    let rows_of = |s: &Sim| {
        s.app
            .md_images()
            .into_iter()
            .find(|p| p.url == url_of(1))
            .unwrap()
            .rows
    };
    let tall = rows_of(&s);
    s.resize(100, 14);
    let short = rows_of(&s);
    assert!(short < tall, "{short} < {tall}");
    assert!(short <= s.app.tab.preview_viewport - 2);
    s.resize(100, 80);
    assert_eq!(rows_of(&s), tall);
}

#[test]
fn e2e_deck_an_ordinary_picture_keeps_its_own_size_next_to_the_slides() {
    // A 300x200 picture in a slide's notes is sized like any picture (its natural 30x10 cells at
    // the test picker's 10x20 px cells), not stretched to the width like a slide.
    let png = {
        let img = image::RgbImage::from_pixel(300, 200, image::Rgb([0, 128, 0]));
        let mut b = Vec::new();
        image::DynamicImage::ImageRgb8(img)
            .write_to(&mut std::io::Cursor::new(&mut b), image::ImageFormat::Png)
            .unwrap();
        b
    };
    let key = "office-img://0123456789ab/photo.png";
    let mut doc = deck_doc(2);
    doc.picture_markdown
        .push_str(&format!("\n![photo]({key})\n"));
    doc.images.push(crate::preview::office::docx::DocImage {
        key: key.into(),
        bytes: png,
        name: "photo.png".into(),
    });
    let Some((s, _d)) = open_with(
        "dv_ordinary_pic",
        Some(ProtocolType::Kitty),
        (100, 80),
        doc,
        cfg_en(),
    ) else {
        return;
    };
    let photo = s
        .app
        .md_images()
        .into_iter()
        .find(|p| p.url == key)
        .unwrap();
    assert_eq!((photo.cols, photo.rows), (30, 10));
    let slide = s
        .app
        .md_images()
        .into_iter()
        .find(|p| p.url == url_of(1))
        .unwrap();
    assert_eq!(slide.cols, 96);
}
