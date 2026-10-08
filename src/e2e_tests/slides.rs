//! End-to-end tests of the presentation preview (`PreviewKind::Document` on a `.pptx`):
//! `testdata/office/slides.pptx` / `slides-ja.pptx` opened through the real key path, `J`/`K`
//! between slides, the chip, the hints, the `R` raw view and the neighbours that must not change.

use super::*;

const EN: &str = "slides.pptx";
const JA: &str = "slides-ja.pptx";
/// Slides in both decks (the 6th is hidden and has speaker notes).
const N: usize = 7;
/// Short enough that the deck does not fit one screen.
const SMALL: (u16, u16) = (100, 14);

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

fn deck_sandbox(
    name: &str,
    files: &[&str],
) -> Option<(crate::test_support::TmpDir, std::path::PathBuf)> {
    let dir = sandbox(name);
    for f in files {
        std::fs::copy(testdata(f)?, dir.join(f)).unwrap();
    }
    std::fs::write(dir.join("z.txt"), "after the deck\n").unwrap();
    let root = canon(&dir);
    Some((dir, root))
}

fn open_deck_cfg(
    name: &str,
    file: &str,
    size: (u16, u16),
    cfg: Config,
) -> Option<(Sim, crate::test_support::TmpDir)> {
    let (dir, root) = deck_sandbox(name, &[file])?;
    let mut s = Sim::with_config_sized(&root, cfg, size.0, size.1);
    s.select(file);
    s.enter();
    Some((s, dir))
}

fn open_deck(
    name: &str,
    file: &str,
    size: (u16, u16),
) -> Option<(Sim, crate::test_support::TmpDir)> {
    open_deck_cfg(name, file, size, cfg_en())
}

/// The first row of the document body on screen (below the status row and the frame's top edge).
fn top_row(s: &Sim) -> String {
    screen_row(s, 2)
}

fn footer_text(s: &Sim) -> String {
    crate::ui::preview::footer_hints(&s.app).join(" | ")
}

#[track_caller]
fn at_slide(s: &Sim, n: usize) {
    assert_eq!(s.app.slide_position(), Some((n, N)), "{}", s.screen());
}

/// A copy of `src` whose presentation lists only its first `keep` slides.
fn deck_with_slides(src: &std::path::Path, dst: &std::path::Path, keep: usize) {
    use std::io::{Read, Write};
    let mut zr = zip::ZipArchive::new(std::fs::File::open(src).unwrap()).unwrap();
    let mut zw = zip::ZipWriter::new(std::fs::File::create(dst).unwrap());
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for i in 0..zr.len() {
        let mut f = zr.by_index(i).unwrap();
        let name = f.name().to_string();
        let mut bytes = Vec::new();
        f.read_to_end(&mut bytes).unwrap();
        if name == "ppt/presentation.xml" {
            let xml = String::from_utf8(bytes).unwrap();
            let (a, rest) = xml.split_once("<p:sldIdLst>").unwrap();
            let (list, b) = rest.split_once("</p:sldIdLst>").unwrap();
            let kept: String = list
                .split_inclusive("/>")
                .filter(|p| p.contains("<p:sldId "))
                .take(keep)
                .collect();
            bytes = format!("{a}<p:sldIdLst>{kept}</p:sldIdLst>{b}").into_bytes();
        }
        zw.start_file(name, opts).unwrap();
        zw.write_all(&bytes).unwrap();
    }
    zw.finish().unwrap();
}

// ---------------------------------------------------------------------------------------------
// Opening, the chip, J/K
// ---------------------------------------------------------------------------------------------

#[test]
fn e2e_slides_a_pptx_opens_as_decorated_markdown_with_the_chip() {
    let Some((s, _d)) = open_deck("sl_open", EN, SMALL) else {
        return;
    };
    assert!(s.app.is_document() && s.app.document_ready());
    assert!(!s.app.is_table_preview() && !s.app.is_windowed() && !s.app.is_md_raw());
    s.see("Slide 1: Quarterly review");
    s.dont_see("## Slide 1");
    s.dont_see("can not preview");
    // The very first frame already carries the chip and the hint (the status rows are drawn after
    // the body, whose layout pass finds the headings).
    s.see("slide 1/7");
    assert!(footer_text(&s).contains("J/K:slide"), "{}", footer_text(&s));
    at_slide(&s, 1);
}

#[test]
fn e2e_slides_j_and_k_move_between_slide_headings_and_the_chip_follows() {
    let Some((mut s, _d)) = open_deck("sl_jk", EN, SMALL) else {
        return;
    };
    s.key('J');
    at_slide(&s, 2);
    assert!(top_row(&s).starts_with("Slide 2: Agenda"), "{}", s.screen());
    s.see("slide 2/7");
    s.key('J');
    at_slide(&s, 3);
    assert!(
        top_row(&s).starts_with("Slide 3: Compare"),
        "{}",
        s.screen()
    );
    s.key('K');
    at_slide(&s, 2);
    assert!(top_row(&s).starts_with("Slide 2: Agenda"), "{}", s.screen());
    s.key('K');
    at_slide(&s, 1);
    assert_eq!(s.app.tab.preview_scroll, 0);
    // The first slide: K has nowhere to go.
    s.key('K');
    at_slide(&s, 1);
    assert_eq!(s.app.tab.preview_scroll, 0);
}

#[test]
fn e2e_slides_walking_to_the_end_reaches_the_last_slide_and_stops() {
    let Some((mut s, _d)) = open_deck("sl_end", EN, SMALL) else {
        return;
    };
    for _ in 0..N + 3 {
        s.key('J');
    }
    // The last slide is short, so its heading cannot reach the top: it is still "slide 7/7".
    at_slide(&s, N);
    s.see("slide 7/7");
    s.see("the site");
    let scroll = s.app.tab.preview_scroll;
    s.key('J');
    at_slide(&s, N);
    assert_eq!(
        s.app.tab.preview_scroll, scroll,
        "J at the end does nothing"
    );
    // And K walks back through every slide to the first.
    let mut seen = vec![N];
    for _ in 0..N + 3 {
        s.key('K');
        if let Some((n, _)) = s.app.slide_position() {
            if seen.last() != Some(&n) {
                seen.push(n);
            }
        }
    }
    assert_eq!(*seen.last().unwrap(), 1, "{seen:?}");
    assert!(
        seen.windows(2).all(|w| w[1] < w[0]),
        "never forward: {seen:?}"
    );
    assert_eq!(s.app.tab.preview_scroll, 0);
}

#[test]
fn e2e_slides_k_inside_a_slide_goes_to_its_start_first() {
    let Some((mut s, _d)) = open_deck("sl_k_mid", EN, SMALL) else {
        return;
    };
    s.key('J');
    s.key('J');
    at_slide(&s, 3);
    s.key('j');
    s.key('j');
    at_slide(&s, 3);
    s.key('K');
    at_slide(&s, 3);
    assert!(
        top_row(&s).starts_with("Slide 3: Compare"),
        "{}",
        s.screen()
    );
    s.key('K');
    at_slide(&s, 2);
}

#[test]
fn e2e_slides_the_chip_follows_plain_scrolling() {
    let Some((mut s, _d)) = open_deck("sl_scroll", EN, SMALL) else {
        return;
    };
    at_slide(&s, 1);
    for _ in 0..8 {
        s.key('j');
    }
    let (n, _) = s.app.slide_position().unwrap();
    assert!(n >= 2, "scrolling down 8 rows leaves slide 1: {n}");
    s.key('g');
    at_slide(&s, 1);
    s.key('G');
    at_slide(&s, N);
}

#[test]
fn e2e_slides_the_hidden_slide_is_marked_counted_and_shows_its_notes() {
    let Some((mut s, _d)) = open_deck("sl_hidden", EN, SMALL) else {
        return;
    };
    for _ in 0..5 {
        s.key('J');
    }
    at_slide(&s, 6);
    assert!(
        top_row(&s).starts_with("Slide 6: Backup (hidden)"),
        "{}",
        s.screen()
    );
    s.see("slide 6/7");
    s.see("Hidden detail");
    // The speaker notes come as a quote under the slide, labelled.
    let md = s.app.document_markdown_for_test().unwrap().to_string();
    assert!(md.contains("> **Notes**"), "{md}");
    s.key('j');
    s.key('j');
    s.key('j');
    s.see("Notes");
    s.see("Say this aloud");
}

#[test]
fn e2e_slides_a_japanese_deck_reads_in_japanese_and_has_a_japanese_chip() {
    let Some((mut s, _d)) = open_deck_cfg("sl_ja", JA, SMALL, cfg_ja()) else {
        return;
    };
    assert!(s.app.document_ready());
    see_cjk(&mut s, "スライド 1/7");
    see_cjk(&mut s, "スライド 1:");
    s.key('J');
    see_cjk(&mut s, "スライド 2/7");
    assert!(s.app.slide_position() == Some((2, N)), "{}", s.screen());
    let f = footer_text(&s);
    assert!(f.contains("J/K:スライド"), "{f}");
    for _ in 0..5 {
        s.key('J');
    }
    assert!(s
        .app
        .document_markdown_for_test()
        .unwrap()
        .contains("非表示"));
}

#[test]
fn e2e_slides_the_pptx_is_never_written_by_any_key() {
    let Some((mut s, d)) = open_deck("sl_nowrite", EN, SMALL) else {
        return;
    };
    let path = d.join(EN);
    let before = std::fs::read(&path).unwrap();
    for k in ['J', 'K', 'J', 'R', 'J', 'K', 'R', ' ', 'o', 'j', 'G', 'g'] {
        s.key(k);
        if s.app.is_outline() {
            s.key('q');
        }
    }
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

// ---------------------------------------------------------------------------------------------
// R: the converted Markdown
// ---------------------------------------------------------------------------------------------

#[test]
fn e2e_slides_r_shows_the_converted_markdown_and_j_k_jump_to_its_headings() {
    let Some((mut s, _d)) = open_deck("sl_raw", EN, SMALL) else {
        return;
    };
    s.key('J'); // decorated: slide 2
    s.key('R');
    assert!(s.app.is_md_raw() && s.app.is_windowed());
    s.see("raw source");
    s.see("## Slide ");
    // The raw view starts where the converted text starts, and has the chip and the hint too.
    s.key('g');
    at_slide(&s, 1);
    assert!(footer_text(&s).contains("J/K:slide"), "{}", footer_text(&s));
    s.key('J');
    at_slide(&s, 2);
    assert_eq!(
        top_row(&s),
        "## Slide 2: Agenda \\*and\\* more",
        "{}",
        s.screen()
    );
    s.key('J');
    s.key('J');
    at_slide(&s, 4);
    assert!(
        top_row(&s).starts_with("## Slide 4: Numbers"),
        "{}",
        s.screen()
    );
    s.key('K');
    at_slide(&s, 3);
    for _ in 0..N + 3 {
        s.key('J');
    }
    at_slide(&s, N);
    s.key('J');
    at_slide(&s, N);
    for _ in 0..N + 3 {
        s.key('K');
    }
    at_slide(&s, 1);
    // The text the raw view reads is the converted Markdown, not the pptx.
    let tmp = s.app.document_raw_file_for_test().expect("a temp file");
    assert_eq!(
        std::fs::read_to_string(&tmp).unwrap(),
        s.app.document_markdown_for_test().unwrap()
    );
    // Back to the rendered view: the chip is still there, J/K work on the rendered text again.
    s.key('R');
    assert!(!s.app.is_md_raw());
    assert!(!tmp.exists());
    at_slide(&s, 1);
    s.key('J');
    at_slide(&s, 2);
    assert!(top_row(&s).starts_with("Slide 2: Agenda"), "{}", s.screen());
}

// ---------------------------------------------------------------------------------------------
// Hints appear only for keys that act ([[hint-shown-iff-key-acts]])
// ---------------------------------------------------------------------------------------------

#[test]
fn e2e_slides_help_lists_the_slide_keys_for_a_presentation_only() {
    let Some((mut s, _d)) = open_deck("sl_help", EN, SMALL) else {
        return;
    };
    s.key('?');
    s.see("next / previous slide");
    s.key('?');
    // A Word document and a plain Markdown file: no such row, no such hint, and J/K do nothing.
    let dir = sandbox("sl_help_others");
    if let Some(w) = testdata("word.docx") {
        std::fs::copy(w, dir.join("word.docx")).unwrap();
    }
    std::fs::write(dir.join("a.md"), "# One\n\ntext\n\n# Two\n\nmore\n").unwrap();
    for f in ["word.docx", "a.md"] {
        if !dir.join(f).exists() {
            continue;
        }
        let mut s = Sim::with_config_sized(&canon(&dir), cfg_en(), SMALL.0, SMALL.1);
        s.select(f);
        s.enter();
        assert!(
            !s.app.slide_can_turn() && s.app.slide_position().is_none(),
            "{f}"
        );
        assert!(!footer_text(&s).contains("J/K"), "{f}: {}", footer_text(&s));
        let scroll = s.app.tab.preview_scroll;
        s.key('J');
        assert_eq!(s.app.tab.preview_scroll, scroll, "{f}");
        s.see_no_help_row();
    }
}

impl Sim {
    /// `?` lists no slide row.
    #[track_caller]
    fn see_no_help_row(&mut self) {
        self.key('?');
        self.dont_see("previous slide");
        self.key('?');
    }
}

#[test]
fn e2e_slides_a_one_slide_deck_offers_no_slide_keys_and_j_k_do_nothing() {
    let Some(src) = testdata(EN) else {
        return;
    };
    let dir = sandbox("sl_one");
    deck_with_slides(&src, &dir.join("one.pptx"), 1);
    let mut s = Sim::with_config_sized(&canon(&dir), cfg_en(), SMALL.0, SMALL.1);
    s.select("one.pptx");
    s.enter();
    assert!(s.app.document_ready());
    s.see("Slide 1: Quarterly review");
    assert!(!s.app.slide_can_turn());
    assert!(!footer_text(&s).contains("J/K"), "{}", footer_text(&s));
    // The chip still tells where the view is (1/1).
    assert_eq!(s.app.slide_position(), Some((1, 1)));
    s.key('J');
    s.key('K');
    assert_eq!(s.app.tab.preview_scroll, 0);
    s.key('?');
    s.dont_see("previous slide");
}

#[test]
fn e2e_slides_a_deck_that_fits_the_screen_keeps_the_hint_and_the_chip() {
    let Some((mut s, _d)) = open_deck("sl_tall", EN, (100, 150)) else {
        return;
    };
    at_slide(&s, 1);
    assert!(s.app.slide_can_turn());
    // Nothing scrolls (the whole deck is on screen), but the keys are still offered and harmless.
    s.key('J');
    assert_eq!(s.app.tab.preview_scroll, 0);
}

#[test]
fn e2e_slides_a_loading_deck_offers_nothing_and_j_does_nothing() {
    let Some((dir, root)) = deck_sandbox("sl_loading", &[EN]) else {
        return;
    };
    let mut s = Sim::with_config_sized(&root, cfg_en(), SMALL.0, SMALL.1).with_media();
    s.select(EN);
    s.enter();
    assert!(s.app.is_document_loading());
    assert!(!s.app.slide_can_turn() && s.app.slide_position().is_none());
    assert!(!footer_text(&s).contains("J/K"), "{}", footer_text(&s));
    s.key('J');
    s.key('K');
    assert!(s.app.is_document_loading());
    s.see("loading");
    s.drain_media();
    assert!(s.app.document_ready());
    at_slide(&s, 1);
    s.key('J');
    at_slide(&s, 2);
    drop(dir);
}

#[test]
fn e2e_slides_a_broken_deck_says_so_and_j_k_do_nothing() {
    let dir = sandbox("sl_broken");
    std::fs::write(dir.join("bad.pptx"), b"this is not a zip").unwrap();
    let mut s = Sim::with_config_sized(&canon(&dir), cfg_en(), 110, 20);
    s.select("bad.pptx");
    s.enter();
    assert!(s.app.is_document() && !s.app.document_ready());
    s.see("[presentation] cannot preview");
    s.see("PowerPoint");
    s.dont_see("Word");
    assert!(!s.app.slide_can_turn() && s.app.slide_position().is_none());
    assert!(!footer_text(&s).contains("J/K"), "{}", footer_text(&s));
    s.key('J');
    s.key('K');
    s.key('R');
    assert!(!s.app.is_md_raw(), "a failed deck has no raw view");
    s.see("[presentation] cannot preview");
}

#[test]
fn e2e_slides_failures_are_japanese_in_a_japanese_ui() {
    let dir = sandbox("sl_broken_ja");
    std::fs::write(dir.join("bad.pptx"), b"this is not a zip").unwrap();
    let mut s = Sim::with_config_sized(&canon(&dir), cfg_ja(), 140, 20);
    s.select("bad.pptx");
    s.enter();
    see_cjk(&mut s, "[プレゼン] 表示不可");
    see_cjk(&mut s, "PowerPoint");
}

#[test]
fn e2e_slides_an_old_binary_ppt_renamed_pptx_is_told_apart_from_a_damaged_file() {
    let dir = sandbox("sl_old_ppt");
    let mut old = vec![0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];
    old.extend_from_slice(&[0u8; 600]);
    std::fs::write(dir.join("old.pptx"), &old).unwrap();
    std::fs::write(dir.join("old2.pptx"), b"PK\x03\x04 truncated").unwrap();
    let mut s = Sim::with_config_sized(&canon(&dir), cfg_en(), 140, 20);
    s.select("old.pptx");
    s.enter();
    s.see("not a PowerPoint format konoma reads");
    s.see("old .ppt");
    s.key('q');
    s.select("old2.pptx");
    s.enter();
    s.see("damaged or not a valid PowerPoint");
}

// ---------------------------------------------------------------------------------------------
// e, rules, tabs, session, [keys]
// ---------------------------------------------------------------------------------------------

#[test]
fn e2e_slides_e_opens_the_office_app_in_both_views_and_never_the_editor() {
    let Some((mut s, _d)) = open_deck("sl_e", EN, SMALL) else {
        return;
    };
    let log = office_recorder(&mut s, 0);
    s.key('e');
    assert!(
        s.app.take_pending_edit().is_none(),
        "must not reach the editor"
    );
    s.key('R');
    s.key('e');
    assert!(s.app.take_pending_edit().is_none());
    let log = log.lock().unwrap();
    assert_eq!(log.len(), 2);
    for (_, args) in log.iter() {
        assert!(args.last().unwrap().ends_with("slides.pptx"), "{args:?}");
    }
}

#[test]
fn e2e_slides_every_presentation_extension_opens_and_the_old_ppt_does_not() {
    let Some(src) = testdata(EN) else {
        return;
    };
    let dir = sandbox("sl_exts");
    let exts = ["pptx", "pptm", "ppsx", "ppsm", "potx", "potm", "UP.PPTX"];
    for e in exts {
        std::fs::copy(&src, dir.join(format!("d.{e}"))).unwrap();
    }
    std::fs::write(dir.join("old.ppt"), b"\xd0\xcf\x11\xe0 old binary").unwrap();
    let root = canon(&dir);
    for e in exts {
        let mut s = Sim::with_config_sized(&root, cfg_en(), SMALL.0, SMALL.1);
        s.select(&format!("d.{e}"));
        s.enter();
        // The reader decides by what is inside, not by the extension: all of them open.
        assert!(s.app.is_document(), "{e}");
        assert!(
            s.app.document_ready() || s.app.document_error().is_some(),
            "{e}"
        );
    }
    let mut s = Sim::with_config_sized(&root, cfg_en(), SMALL.0, SMALL.1);
    s.select("old.ppt");
    s.enter();
    assert!(!s.app.is_document(), ".ppt is not previewed");
    // A user rule placed first replaces the built-in.
    let mut cfg = cfg_en();
    cfg.preview.rules.insert(
        0,
        crate::config::Rule {
            glob: Some("*.pptx".into()),
            builtin: Some("text".into()),
            ..crate::config::Rule::default()
        },
    );
    let mut s = Sim::with_config_sized(&root, cfg, SMALL.0, SMALL.1);
    s.select("d.pptx");
    s.enter();
    assert!(!s.app.is_document());
}

#[test]
fn e2e_slides_each_tab_keeps_its_own_slide_and_deck() {
    let Some((dir, root)) = deck_sandbox("sl_tabs", &[EN, JA]) else {
        return;
    };
    let mut s = Sim::with_config_sized(&root, cfg_en(), SMALL.0, SMALL.1);
    s.select(EN);
    s.enter();
    s.key('J');
    s.key('J');
    at_slide(&s, 3);
    s.key('t');
    assert!(s.app.slide_position().is_none(), "the new tab is a tree");
    s.select(JA);
    s.enter();
    at_slide(&s, 1);
    s.key('J');
    at_slide(&s, 2);
    s.key('1');
    at_slide(&s, 3);
    assert!(
        top_row(&s).starts_with("Slide 3: Compare"),
        "{}",
        s.screen()
    );
    s.key('2');
    at_slide(&s, 2);
    see_cjk(&mut s, "議題");
    drop(dir);
}

#[test]
fn e2e_slides_the_session_restores_the_deck_at_its_first_slide_with_working_keys() {
    let Some((dir, root)) = deck_sandbox("sl_session", &[EN]) else {
        return;
    };
    let base = unique_tmp("konoma_e2e_slides_session_base");
    let _ = std::fs::remove_dir_all(&base);
    let mut s = Sim::with_config_sized(&root, cfg_en(), SMALL.0, SMALL.1);
    s.app
        .attach_session_store(crate::session::SessionStore::with_base(
            base.to_path_buf(),
            &root,
        ));
    s.select(EN);
    s.enter();
    s.key('J');
    s.key('t');
    s.app.save_session();
    drop(s);

    let mut s2 = Sim::with_config_sized(&root, cfg_en(), SMALL.0, SMALL.1);
    s2.app
        .attach_session_store(crate::session::SessionStore::with_base(
            base.to_path_buf(),
            &root,
        ));
    s2.app.restore_session();
    s2.draw();
    assert_eq!(s2.app.tab_count(), 2);
    s2.key('[');
    assert!(s2.app.is_document() && s2.app.document_ready());
    // Whatever scroll the restore gives, the chip agrees with the view and the keys work.
    let (_, total) = s2.app.slide_position().expect("a chip");
    assert_eq!(total, N);
    s2.key('g');
    at_slide(&s2, 1);
    s2.key('J');
    at_slide(&s2, 2);
    drop(dir);
}

#[test]
fn e2e_slides_keys_config_rebinds_the_slide_keys_with_the_new_and_old_names() {
    for (next, prev) in [
        ("page_next", "page_prev"),
        ("pdf_next_page", "pdf_prev_page"),
    ] {
        let cfg = cfg_keys("preview_text", &[("L", next), ("H", prev), ("J", "noop")]);
        let Some((mut s, _d)) = open_deck_cfg("sl_keys", EN, SMALL, cfg) else {
            return;
        };
        s.key('J'); // unbound by the config
        at_slide(&s, 1);
        s.key('L');
        at_slide(&s, 2);
        s.key('H');
        at_slide(&s, 1);
    }
}

#[test]
fn e2e_slides_page_down_and_the_scroll_keys_keep_their_meaning() {
    let Some((mut s, _d)) = open_deck("sl_pgdn", EN, SMALL) else {
        return;
    };
    s.press(KeyCode::PageDown, KeyModifiers::NONE);
    assert!(s.app.tab.preview_scroll > 0, "PageDown still scrolls");
    s.key('g');
    s.press(KeyCode::Char('d'), KeyModifiers::CONTROL);
    assert!(s.app.tab.preview_scroll > 0, "Ctrl-d still scrolls");
}

#[test]
fn e2e_slides_ctrl_n_pages_to_the_next_file_and_releases_the_deck() {
    let Some((mut s, _d)) = open_deck("sl_ctrl_n", EN, SMALL) else {
        return;
    };
    s.press(KeyCode::Char('n'), KeyModifiers::CONTROL);
    s.see("after the deck");
    assert!(!s.app.is_document() && s.app.slide_position().is_none());
    s.key('J');
    assert!(s.app.slide_position().is_none());
}

#[test]
fn e2e_slides_an_outside_edit_reloads_the_deck_and_the_chip_stays_true() {
    let Some((dir, root)) = deck_sandbox("sl_reload", &[EN]) else {
        return;
    };
    let mut s = Sim::with_config_sized(&root, cfg_en(), SMALL.0, SMALL.1).with_media();
    s.select(EN);
    s.enter();
    s.drain_media();
    s.key('J');
    s.key('J');
    at_slide(&s, 3);
    // The file is replaced by a 2-slide version of itself, saved by an outside program.
    let path = root.join(EN);
    let src = testdata(EN).unwrap();
    deck_with_slides(&src, &path, 2);
    std::fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_900_000_200))
        .unwrap();
    s.app.refresh_fs_watched(false, std::slice::from_ref(&path));
    s.draw();
    drain_media_until_current(&mut s);
    assert!(s.app.document_ready());
    let (n, total) = s.app.slide_position().expect("a chip");
    assert_eq!(total, 2, "{}", s.screen());
    assert!((1..=2).contains(&n));
    s.key('g');
    s.key('J');
    assert_eq!(s.app.slide_position(), Some((2, 2)));
    drop(dir);
}

#[test]
fn e2e_slides_k_never_gets_stuck_when_the_last_slides_share_the_final_screen() {
    // 20 rows: slides 6 and 7 both fit on the last screen, so neither heading can reach the top.
    let Some((mut s, _d)) = open_deck("sl_trap", EN, (100, 20)) else {
        return;
    };
    for _ in 0..N + 2 {
        s.key('J');
    }
    at_slide(&s, N);
    let mut last = s.app.tab.preview_scroll;
    assert!(last > 0);
    for i in 0..N + 2 {
        s.key('K');
        let now = s.app.tab.preview_scroll;
        if last == 0 {
            break;
        }
        assert!(
            now < last,
            "press {i}: K must move up ({last} -> {now})\n{}",
            s.screen()
        );
        last = now;
    }
    at_slide(&s, 1);
    assert_eq!(s.app.tab.preview_scroll, 0);
}
