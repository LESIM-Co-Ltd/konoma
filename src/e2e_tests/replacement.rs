//! A plugin installed under **another id** is drawn the way the standard plugin of the same display
//! type is. The host chooses a screen from what a plugin declares (`Manifest::shows` and friends)
//! and from the view it returns, never from its id, so replacing a standard plugin (a different id,
//! the standard one switched off with `enabled = false`, design §5) changes nothing the user sees
//! as long as the replacement returns the same display type.
//!
//! Each test registers stand-ins from `plugins::test_views` the way the stage-3 registration will
//! (`plugins::test_plugins`), never in the shipped registry. Where a standard plugin of the same
//! shape can be run on a file, the two screens are compared; for the rest (pages, representations)
//! the keys are driven and the result is read off the app.

use super::*;
use crate::plugins::test_plugins;
use crate::plugins::test_plugins::{Hexdump, Variant};
use crate::plugins::test_views::{
    Broken, Deck, DocConverted, DocFile, Notifier, Pic, Picture, Styled, TableLike,
};
use konoma_plugin::Plugin;
use std::sync::Arc;

/// A config in which the stand-ins are registered (a marker each) and the standard plugins
/// `disabled` are switched off.
fn cfg_with(stand_ins: &[(&str, Arc<dyn Plugin>)], disabled: &[&str]) -> Config {
    let mut cfg = Config::default();
    cfg.ui.lang = "en".into();
    for (name, plugin) in stand_ins {
        let marker = test_plugins::register(name, plugin.clone());
        cfg.plugins.insert(marker, off());
    }
    for id in disabled {
        cfg.plugins.insert((*id).to_string(), off());
    }
    cfg
}

fn off() -> toml::Table {
    let mut t = toml::Table::new();
    t.insert("enabled".into(), toml::Value::Boolean(false));
    t
}

/// Two roots with names of the same length (`std` and `new`) under one sandbox, so the screens of
/// two sims can be compared: the title of the frame shows the root's name.
fn pair(
    name: &str,
) -> (
    crate::test_support::TmpDir,
    std::path::PathBuf,
    std::path::PathBuf,
) {
    let parent = sandbox(name);
    let (a, b) = (parent.join("std"), parent.join("new"));
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    (parent, canon(&a), canon(&b))
}

/// The screen as it is (the roots of a `pair` differ only in the letters of their names).
fn plain(s: &Sim, _dir: &std::path::Path) -> String {
    s.screen()
        .replace("┌ std/", "┌ XXX/")
        .replace("┌ new/", "┌ XXX/")
}

/// The id of the plugin showing the preview.
fn shown_by(s: &Sim) -> String {
    match &s.app.tab.preview_kind {
        Some(crate::preview::PreviewKind::Plugin { id, .. }) => id.clone(),
        other => panic!("not a plugin's preview: {other:?}"),
    }
}

/// Drive the same keys on two sims and require the same screen after each.
fn same_screens(
    (a, da): (&mut Sim, &std::path::Path),
    (b, db): (&mut Sim, &std::path::Path),
    steps: &[&str],
) {
    for step in steps {
        for c in step.chars() {
            match c {
                '\n' => {
                    a.enter();
                    b.enter();
                }
                '\u{1b}' => {
                    a.esc();
                    b.esc();
                }
                '\t' => {
                    a.tab();
                    b.tab();
                }
                c => {
                    a.key(c);
                    b.key(c);
                }
            }
        }
        a.draw();
        b.draw();
        assert_eq!(
            plain(a, da),
            plain(b, db),
            "the screens differ after {step:?}"
        );
    }
}

// ---- a document -----------------------------------------------------------------------------------

const MARKDOWN: &str = "# Title\n\nSome **bold** words and a [link](https://example.com).\n\n- one\n- two\n\n```rust\nfn main() {}\n```\n\n| a | b |\n|---|---|\n| 1 | 2 |\n";

#[test]
fn a_document_plugin_under_another_id_is_drawn_like_the_markdown_plugin() {
    let (_parent, std_dir, new_dir) = pair("repl_doc");
    for d in [&std_dir, &new_dir] {
        std::fs::write(d.join("doc.md"), MARKDOWN).unwrap();
    }
    let new_cfg = cfg_with(
        &[(
            "repl_doc",
            Arc::new(DocFile {
                id: "x-markdown",
                ext: "md",
                takes_markdown: true,
            }),
        )],
        &["markdown"],
    );
    let (sd, nd) = (std_dir, new_dir);
    let mut a = Sim::with_config(&sd, {
        let mut c = Config::default();
        c.ui.lang = "en".into();
        c
    });
    let mut b = Sim::with_config(&nd, new_cfg);
    for s in [&mut a, &mut b] {
        s.select("doc.md");
        s.enter();
    }
    assert_eq!(shown_by(&a), "markdown");
    assert_eq!(shown_by(&b), "x-markdown");
    assert!(b.app.document_ready() && b.app.holds_document());
    b.see("Title");
    b.dont_see("# Title");
    // The rendered screen, the source (`R`), back, a search, a focus on the link, the outline.
    same_screens(
        (&mut a, &sd),
        (&mut b, &nd),
        &["", "R", "R", "/bold\n", "\u{1b}", "\t", "o"],
    );
}

#[test]
fn a_document_that_a_plugin_converts_is_handed_to_the_plugin_that_takes_markdown() {
    // Word's shape: the plugin answers "open this again as Markdown"; the standard markdown plugin
    // (or any plugin that takes the content type) draws it. The Markdown here is the standard one.
    let dir = sandbox("repl_conv");
    std::fs::write(dir.join("a.xconv"), "# Heading\n\nconverted *text*\n").unwrap();
    let cfg = cfg_with(
        &[(
            "repl_conv",
            Arc::new(DocConverted {
                id: "x-conv",
                ext: "xconv",
            }),
        )],
        &[],
    );
    let mut s = Sim::with_config(&canon(&dir), cfg);
    s.select("a.xconv");
    s.enter();
    assert_eq!(shown_by(&s), "x-conv");
    assert!(s.app.document_ready());
    s.see("Heading");
    s.dont_see("# Heading");
    // The converted text has no file to be the source of: `R` shows it from a generated one.
    s.key('R');
    s.see("# Heading");
    assert!(s.app.is_raw_source());
    s.key('R');
    s.dont_see("# Heading");
}

#[test]
fn a_replacement_of_the_markdown_plugin_also_draws_what_a_converted_document_hands_over() {
    let dir = sandbox("repl_conv_md");
    std::fs::write(dir.join("a.xconv"), "# Heading\n\nconverted *text*\n").unwrap();
    let cfg = cfg_with(
        &[
            (
                "repl_conv_md_a",
                Arc::new(DocConverted {
                    id: "x-conv",
                    ext: "xconv",
                }),
            ),
            (
                "repl_conv_md_b",
                Arc::new(DocFile {
                    id: "x-markdown",
                    ext: "md",
                    takes_markdown: true,
                }),
            ),
        ],
        &["markdown"],
    );
    let mut s = Sim::with_config(&canon(&dir), cfg);
    s.select("a.xconv");
    s.enter();
    assert!(s.app.document_ready());
    s.see("Heading");
    s.dont_see("# Heading");
}

// ---- a deck: pages that start at headings ------------------------------------------------------

#[test]
fn a_deck_under_another_id_turns_pages_with_j_and_k() {
    let dir = sandbox("repl_deck");
    std::fs::write(
        dir.join("a.xdeck"),
        "# One\n\nfirst\n\n# Two\n\nsecond\n\n# Three\n\nthird\n",
    )
    .unwrap();
    let cfg = cfg_with(
        &[(
            "repl_deck",
            Arc::new(Deck {
                id: "x-deck",
                ext: "xdeck",
            }),
        )],
        &[],
    );
    let mut s = Sim::with_config_sized(&canon(&dir), cfg, 80, 12);
    s.select("a.xdeck");
    s.enter();
    assert_eq!(shown_by(&s), "x-deck");
    assert!(s.app.is_deck(), "the layout says the pages are anchored");
    assert_eq!(s.app.slide_position(), Some((1, 3)));
    s.key('J');
    assert_eq!(s.app.slide_position(), Some((2, 3)));
    s.key('J');
    assert_eq!(s.app.slide_position(), Some((3, 3)));
    s.key('J');
    assert_eq!(s.app.slide_position(), Some((3, 3)), "stops at the last");
    s.key('K');
    assert_eq!(s.app.slide_position(), Some((2, 3)));
    // `O` lists the pages.
    assert!(s.app.is_page_list_preview());
    // `R` switches between the representations the plugin declares, keeping the page.
    assert_eq!(s.app.deck_view_choice_for_test(), None, "the default");
    s.key('R');
    assert_eq!(s.app.deck_view_choice_for_test(), Some(false), "pictures");
    s.key('R');
    assert_eq!(s.app.deck_view_choice_for_test(), Some(true), "text");
    assert_eq!(s.app.slide_position(), Some((2, 3)), "the page is kept");
}

// ---- a table ------------------------------------------------------------------------------------

#[test]
fn a_table_plugin_under_another_id_is_drawn_like_the_csv_plugin() {
    let (_parent, std_dir, new_dir) = pair("repl_csv");
    for d in [&std_dir, &new_dir] {
        std::fs::write(d.join("t.csv"), "name,qty\napple,3\npear,12\nplum,7\n").unwrap();
    }
    let new_cfg = cfg_with(
        &[("repl_csv", Arc::new(TableLike::new("x-csv", "csv", false)))],
        &["csv"],
    );
    let (sd, nd) = (std_dir, new_dir);
    let mut std_cfg = Config::default();
    std_cfg.ui.lang = "en".into();
    let mut a = Sim::with_config(&sd, std_cfg);
    let mut b = Sim::with_config(&nd, new_cfg);
    for s in [&mut a, &mut b] {
        s.select("t.csv");
        s.enter();
    }
    assert_eq!(shown_by(&b), "x-csv");
    assert!(b.app.is_table_preview() && b.app.grid().is_some());
    // The table drawn, the cell cursor moved, a search, the cell popup.
    same_screens(
        (&mut a, &sd),
        (&mut b, &nd),
        &["", "jl", "/12\n", "\u{1b}", "\n"],
    );
}

#[test]
fn a_table_with_pages_under_another_id_turns_sheets_and_searches_and_copies() {
    let dir = sandbox("repl_sheet");
    std::fs::write(
        dir.join("b.xsheet"),
        "a,b\n1,2\n---\nc,d\n3,4\n---\ne,f\n5,6",
    )
    .unwrap();
    let cfg = cfg_with(
        &[(
            "repl_sheet",
            Arc::new(TableLike::new("x-sheet", "xsheet", true)),
        )],
        &[],
    );
    let mut s = Sim::with_config(&canon(&dir), cfg);
    s.select("b.xsheet");
    s.enter();
    assert_eq!(shown_by(&s), "x-sheet");
    assert!(
        s.app.is_spreadsheet_preview(),
        "a table that converts, with pages"
    );
    assert_eq!(
        s.app.sheet_info().map(|(n, i, t)| (n.to_string(), i, t)),
        Some(("Sheet1".into(), 1, 3))
    );
    s.see("1");
    s.key('J');
    assert_eq!(
        s.app.sheet_info().map(|(n, i, t)| (n.to_string(), i, t)),
        Some(("Sheet2".into(), 2, 3))
    );
    // The cursor, a search over the cells of the sheet, the copy of a cell.
    s.keys("jl");
    crate::test_support::clear_test_clipboard();
    s.keys("yc");
    assert_eq!(
        crate::test_support::get_test_clipboard().as_deref(),
        Some("4"),
        "the cell under the cursor"
    );
    s.key('K');
    assert_eq!(s.app.sheet_info().map(|(_, i, _)| i), Some(1));
    // `O` lists the sheets by their names.
    assert!(s.app.is_page_list_preview());
}

// ---- a picture ---------------------------------------------------------------------------------

#[test]
fn a_picture_plugin_with_pages_under_another_id_turns_pages() {
    let dir = sandbox("repl_pic");
    std::fs::write(dir.join("p.xpic"), b"x").unwrap();
    let cfg = cfg_with(
        &[(
            "repl_pic",
            Arc::new(Picture {
                id: "x-pic",
                ext: "xpic",
                pic: Pic::Pages(3),
            }),
        )],
        &[],
    );
    let mut s = Sim::with_config(&canon(&dir), cfg).with_media();
    s.select("p.xpic");
    s.enter();
    s.drain_media();
    assert_eq!(shown_by(&s), "x-pic");
    assert!(s.app.is_image_preview());
    let first = |s: &Sim| s.app.shown_picture_rgba().expect("a picture").2[..4].to_vec();
    assert_eq!(first(&s), vec![255, 0, 0, 255], "page 1 is red");
    s.key('J');
    s.drain_media();
    assert_eq!(first(&s), vec![0, 255, 0, 255], "page 2 is green");
    s.key('J');
    s.drain_media();
    assert_eq!(first(&s), vec![0, 0, 255, 255], "page 3 is blue");
    s.key('K');
    s.drain_media();
    assert_eq!(first(&s), vec![0, 255, 0, 255]);
}

// ---- text ---------------------------------------------------------------------------------------

#[test]
fn a_text_plugin_that_makes_its_text_gets_the_host_windowed_reader() {
    let dir = sandbox("repl_text");
    std::fs::write(dir.join("a.bin"), (0u8..40).collect::<Vec<_>>()).unwrap();
    let cfg = cfg_with(&[("repl_text", Arc::new(Hexdump(Variant::default())))], &[]);
    let mut s = Sim::with_config_sized(&canon(&dir), cfg, 80, 12);
    s.select("a.bin");
    s.enter();
    assert!(s.app.is_windowed(), "the made text is windowed like a file");
    s.see("00 01 02 03 04 05 06 07 08 09 0a 0b 0c 0d 0e 0f");
    // The caret moves line by line.
    assert_eq!(s.app.preview_caret_line(), 0);
    s.key('j');
    assert_eq!(s.app.preview_caret_line(), 1);
    // `/` searches the lines.
    s.key('/');
    s.keys("20 21");
    s.enter();
    assert_eq!(
        s.app.search_status(),
        Some((1, 1)),
        "one match, the third row"
    );
    // `V` and `y` copy the line under the caret (the search does not move it, as in a file).
    crate::test_support::clear_test_clipboard();
    s.key('V');
    assert!(s.app.is_preview_visual() && s.app.preview_visual_linewise());
    s.key('y');
    let copied = crate::test_support::get_test_clipboard().unwrap_or_default();
    assert!(
        copied.starts_with("10 11 12 13") && !copied.contains("20 21"),
        "the second row (under the caret) was copied: {copied:?}"
    );
    // `e` has no line of the file to open at: these lines are not the file's.
    assert_eq!(s.app.preview_edit_line_for_test(), None);
}

#[test]
fn the_colors_a_text_plugin_returns_are_drawn_in_the_windowed_reader() {
    let dir = sandbox("repl_styled");
    std::fs::write(dir.join("a.xtxt"), "plain one\n!! red two\nplain three\n").unwrap();
    let cfg = cfg_with(
        &[(
            "repl_styled",
            Arc::new(Styled {
                id: "x-styled",
                ext: "xtxt",
            }),
        )],
        &[],
    );
    let mut s = Sim::with_config_sized(&canon(&dir), cfg, 80, 12);
    s.select("a.xtxt");
    s.enter();
    assert!(s.app.is_windowed());
    s.see("red two");
    s.dont_see("!! red");
    s.see_styled(
        "red two",
        |st| st.fg == Some(ratatui::style::Color::Red),
        "red",
    );
    s.see_styled(
        "plain one",
        |st| st.fg != Some(ratatui::style::Color::Red),
        "not red",
    );
}

// ---- a plugin that reads heavily is not read again for an event that changed nothing -----------------

#[test]
fn a_heavy_plugin_is_read_again_only_when_its_file_changed() {
    let dir = sandbox("repl_heavy");
    let file = dir.join("big.xsheet");
    std::fs::write(&file, "a,b\n1,2\n").unwrap();
    let plugin = TableLike {
        heavy: true,
        ..TableLike::new("x-heavy", "xsheet", true)
    };
    let opens = plugin.opens.clone();
    let cfg = cfg_with(&[("repl_heavy", Arc::new(plugin))], &[]);
    let mut s = Sim::with_config(&canon(&dir), cfg);
    s.select("big.xsheet");
    s.enter();
    let after_open = opens.load(std::sync::atomic::Ordering::SeqCst);
    assert_eq!(after_open, 1);
    // An event for `.git`, or for another file, reaches `reload_preview` with the file unchanged.
    s.app.reload_preview();
    s.app.reload_preview();
    assert_eq!(
        opens.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "not read again"
    );
    // The file changed (its size and its time): read again.
    std::fs::write(&file, "a,b\n1,2\n3,4\n").unwrap();
    let f = std::fs::OpenOptions::new().write(true).open(&file).unwrap();
    f.set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_900_000_000))
        .unwrap();
    s.app.reload_preview();
    assert_eq!(
        opens.load(std::sync::atomic::Ordering::SeqCst),
        2,
        "read again"
    );
}

#[test]
fn a_light_plugin_is_read_again_by_every_reload_as_it_always_was() {
    let dir = sandbox("repl_light");
    std::fs::write(dir.join("t.csv"), "a,b\n1,2\n").unwrap();
    let plugin = TableLike::new("x-light", "csv", false);
    let opens = plugin.opens.clone();
    let cfg = cfg_with(&[("repl_light", Arc::new(plugin))], &["csv"]);
    let mut s = Sim::with_config(&canon(&dir), cfg);
    s.select("t.csv");
    s.enter();
    assert_eq!(opens.load(std::sync::atomic::Ordering::SeqCst), 1);
    s.app.reload_preview();
    assert_eq!(opens.load(std::sync::atomic::Ordering::SeqCst), 2);
}

// ---- the SVG a plugin returns: who vouches for it is decided by where the plugin comes from ------

#[test]
fn an_svg_a_user_plugin_claims_is_trusted_is_drawn_as_untrusted_and_redrawn_so() {
    use crate::app::SvgTrust;
    let dir = sandbox("repl_svg_trust");
    std::fs::write(dir.join("a.xsvg"), b"x").unwrap();
    std::fs::write(dir.join("b.xsvgd"), b"x").unwrap();
    let cfg = cfg_with(
        &[
            (
                "repl_svg_a",
                Arc::new(Picture {
                    id: "x-svg",
                    ext: "xsvg",
                    pic: Pic::ClaimedTrustedSvg,
                }),
            ),
            (
                "repl_svg_b",
                Arc::new(Picture {
                    id: "x-svg-dir",
                    ext: "xsvgd",
                    pic: Pic::ClaimedTrustedSvgInDir,
                }),
            ),
        ],
        &[],
    );
    let mut s = Sim::with_config(&canon(&dir), cfg).with_media();
    s.select("a.xsvg");
    s.enter();
    s.drain_media();
    assert!(s.app.is_image_preview());
    assert_eq!(
        s.app.vector_trust_for_test(),
        Some(&SvgTrust::Embedded),
        "no directory named: nothing to read, and the child draws it again"
    );
    s.esc();
    s.select("b.xsvgd");
    s.enter();
    s.drain_media();
    assert!(
        matches!(s.app.vector_trust_for_test(), Some(SvgTrust::File(_))),
        "{:?}",
        s.app.vector_trust_for_test()
    );
}

// ---- what the host shows when the plugin shows nothing --------------------------------------------

/// Opens `name` (with `body`) under a plugin of this declaration that always fails, and returns the
/// screen.
fn broken_screen(how: konoma_plugin::Unavailable, lang: &str) -> String {
    let dir = sandbox(&format!("repl_broken_{}", how_name(&how)));
    std::fs::write(dir.join("f.xbad"), "the file's own text\n").unwrap();
    let mut cfg = cfg_with(
        &[(
            &format!("repl_broken_{}", how_name(&how)),
            Arc::new(Broken {
                id: "x-broken",
                ext: "xbad",
                how,
            }),
        )],
        &[],
    );
    cfg.ui.lang = lang.into();
    let mut s = Sim::with_config(&canon(&dir), cfg);
    s.select("f.xbad");
    s.enter();
    s.screen()
}

fn how_name(how: &konoma_plugin::Unavailable) -> &'static str {
    match how {
        konoma_plugin::Unavailable::Message => "message",
        konoma_plugin::Unavailable::RawText { .. } => "raw",
        konoma_plugin::Unavailable::Notice(_) => "notice",
        konoma_plugin::Unavailable::Picture => "picture",
    }
}

#[test]
fn a_plugin_that_cannot_show_the_file_is_shown_the_way_its_manifest_says() {
    use konoma_plugin::Unavailable;
    // Default: the failure's message, then the file name.
    let m = broken_screen(Unavailable::Message, "en");
    assert!(
        m.contains("it is broken beyond repair") && m.contains("f.xbad"),
        "{m}"
    );
    assert!(!m.contains("the file's own text"), "{m}");
    // A table that cannot be read is still text (the CSV plugin's way).
    let t = broken_screen(Unavailable::RawText { with_reason: false }, "en");
    assert!(t.contains("the file's own text"), "{t}");
    // A sentence of its own, in the user's language, whatever the failure said (the archive way).
    let words = konoma_plugin::t!(en: "[x] cannot be listed", ja: "[x] 一覧にできません");
    let n = broken_screen(Unavailable::Notice(words.clone()), "en");
    assert!(
        n.contains("[x] cannot be listed") && n.contains("f.xbad"),
        "{n}"
    );
    assert!(!n.contains("beyond repair"), "{n}");
    let nj = broken_screen(Unavailable::Notice(words), "ja");
    // (Wide glyphs are drawn with gaps in the test terminal: look for what is not them.)
    assert!(
        nj.contains("[x]") && !nj.contains("cannot be listed"),
        "{nj}"
    );
}

#[test]
fn a_plugin_that_answers_with_a_notice_has_it_shown() {
    let dir = sandbox("repl_notice");
    std::fs::write(dir.join("n.xnote"), "x").unwrap();
    let cfg = cfg_with(
        &[(
            "repl_notice",
            Arc::new(Notifier {
                id: "x-notice",
                ext: "xnote",
            }),
        )],
        &[],
    );
    let mut s = Sim::with_config(&canon(&dir), cfg);
    s.select("n.xnote");
    s.enter();
    s.see("Nothing to see here.");
    s.see("Come back later.");
    s.dont_see("can not preview");
}
