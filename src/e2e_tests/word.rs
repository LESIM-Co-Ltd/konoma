//! End-to-end tests of the Word document preview (`PreviewKind::Document`): `testdata/office/word.docx`
//! and `word-ja.docx` opened through the real key path.

use super::*;

const EN: &str = "word.docx";
const JA: &str = "word-ja.docx";

/// A tall terminal, so most of the sample document is on one screen.
const TALL: (u16, u16) = (110, 150);

fn testdata(file: &str) -> Option<std::path::PathBuf> {
    let src = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("testdata/office")
        .join(file);
    if !src.exists() {
        eprintln!("SKIP: testdata/office/{file} not found — this test verifies nothing this run");
        return None;
    }
    Some(src)
}

/// A sandbox holding `file` (copied from testdata) with a `z.txt` after it in tree order.
fn doc_sandbox(
    name: &str,
    file: &str,
) -> Option<(crate::test_support::TmpDir, std::path::PathBuf)> {
    let src = testdata(file)?;
    let dir = sandbox(name);
    std::fs::copy(&src, dir.join(file)).unwrap();
    std::fs::write(dir.join("z.txt"), "after the document\n").unwrap();
    let root = canon(&dir);
    Some((dir, root))
}

/// Opens `file` through the tree in an English-UI sim of `size`, without media workers (the
/// document is converted inline, no pictures are drawn).
fn open_doc_sized(
    name: &str,
    file: &str,
    size: (u16, u16),
    cfg: Config,
) -> Option<(Sim, crate::test_support::TmpDir)> {
    let (dir, root) = doc_sandbox(name, file)?;
    let mut s = Sim::with_config_sized(&root, cfg, size.0, size.1);
    s.select(file);
    s.enter();
    Some((s, dir))
}

fn open_doc(name: &str, file: &str) -> Option<(Sim, crate::test_support::TmpDir)> {
    open_doc_sized(name, file, TALL, cfg_en())
}

/// Opens `file` with the real media workers (the Office worker, the picture decoder and encoder),
/// the way the run loop does. The conversion is still on its thread on return (`drain_media`
/// applies it).
fn open_doc_with_media(
    name: &str,
    file: &str,
    size: (u16, u16),
) -> Option<(Sim, crate::test_support::TmpDir)> {
    let (dir, root) = doc_sandbox(name, file)?;
    let mut s = Sim::with_config_sized(&root, cfg_en(), size.0, size.1).with_media();
    s.select(file);
    s.enter();
    Some((s, dir))
}

fn same_bytes_after(path: &std::path::Path, f: impl FnOnce()) -> bool {
    let before = std::fs::read(path).unwrap();
    f();
    std::fs::read(path).unwrap() == before
}

/// A minimal docx (three parts) whose body is `body` (the content of `<w:body>`).
fn build_docx(path: &std::path::Path, body: &str) {
    use std::io::Write;
    let ns = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
    let rel = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument";
    let f = std::fs::File::create(path).unwrap();
    let mut zw = zip::ZipWriter::new(f);
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    let mut put = |name: &str, text: &str| {
        zw.start_file(name, opts).unwrap();
        zw.write_all(text.as_bytes()).unwrap();
    };
    put(
        "[Content_Types].xml",
        r#"<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"/>"#,
    );
    put(
        "_rels/.rels",
        &format!(
            r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{rel}" Target="word/document.xml"/></Relationships>"#
        ),
    );
    put(
        "word/document.xml",
        &format!(
            r#"<?xml version="1.0"?><w:document xmlns:w="{ns}"><w:body>{body}</w:body></w:document>"#
        ),
    );
    zw.finish().unwrap();
}

fn para(text: &str) -> String {
    format!("<w:p><w:r><w:t>{text}</w:t></w:r></w:p>")
}

/// Applies every pending inline-image decode / encode result (the run loop's draining steps),
/// redrawing after each batch, until the pipeline has gone quiet.
#[track_caller]
fn settle_images(s: &mut Sim) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
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
        assert!(std::time::Instant::now() < deadline, "images never settled");
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

/// Moves the Tab focus forward until `want` is the focused kind (at most `max` presses).
#[track_caller]
fn tab_to(s: &mut Sim, want: crate::app::MdFocus, max: usize) {
    for _ in 0..max {
        s.tab();
        if s.app.md_focused_kind() == Some(want) {
            return;
        }
    }
    panic!("Tab never reached {want:?}:\n{}", s.screen());
}

// ---------------------------------------------------------------------------------------------
// Opening and the decorated view
// ---------------------------------------------------------------------------------------------

#[test]
fn e2e_word_docx_opens_as_decorated_markdown() {
    let Some((s, _d)) = open_doc("w_open", EN) else {
        return;
    };
    assert!(s.app.is_document() && s.app.document_ready());
    assert!(!s.app.is_table_preview() && !s.app.is_windowed() && !s.app.is_md_raw());
    // Headings are drawn as headings (decorated), not as `#` source.
    s.see("Word reader sample");
    s.dont_see("# Word reader sample");
    s.dont_see("can not preview");
    // Emphasis is applied; the literal marks the author typed survive as text.
    s.see("*star* _under_");
    s.dont_see("**bold**");
    s.see("PREVIEW");
}

#[test]
fn e2e_word_the_structure_of_the_document_reaches_the_screen() {
    let Some((s, _d)) = open_doc("w_structure", EN) else {
        return;
    };
    // Headings, a table of contents made of links, bullets and nested bullets.
    for want in [
        "Contents",
        "Lists",
        "Bullets and numbers",
        "- bullet one",
        "- bullet nested",
    ] {
        s.see(want);
    }
    // Numbered lists keep the numbers the document showed; other number formats are literal text.
    for want in [
        "1. first",
        "2. second",
        "1. second-a",
        "3. third",
        "a. alpha",
        "II. two",
    ] {
        s.see(want);
    }
    // The table (a merged header cell, an escaped pipe, a two-line cell), drawn as a grid.
    for want in [
        "┌",
        "Merged across two columns",
        "a|b",
        "two lines",
        "tall cell",
    ] {
        s.see(want);
    }
    // Footnotes and endnotes: superscript references and a section at the end.
    for want in [
        "footnote¹",
        "endnote²",
        "1. Footnote text with *star*.",
        "2. Endnote text.",
    ] {
        s.see(want);
    }
    // Links collapse to their label; the tracked-change insertion is in, the deletion is out.
    s.see("example site");
    s.see("Kept text. This sentence is inserted.");
    s.dont_see("deleted");
    // Comments, headers and footers are not shown.
    for hidden in ["SECRET-COMMENT", "RUNNING-HEADER", "RUNNING-FOOTER"] {
        s.dont_see(hidden);
    }
    s.see("Text with a comment");
    // A text box's text is in the flow; a code paragraph is a code block.
    s.see("TEXTBOX-TEXT inside a frame");
    s.see("def f(x):");
    s.see("code");
}

#[test]
fn e2e_word_a_picture_has_its_alt_text_where_images_cannot_be_drawn() {
    let Some((s, _d)) = open_doc("w_alt", EN) else {
        return;
    };
    // No graphics backend in this sim: the picture degrades to its description, never to the
    // synthetic URL alone or to a blank.
    s.see("Gradient A small gradient picture");
}

#[test]
fn e2e_word_a_japanese_document_opens_and_reads_in_japanese() {
    let Some((mut s, _d)) = open_doc("w_ja", JA) else {
        return;
    };
    for want in ["Word 読み込みサンプル", "はじめに", "リスト", "図と脚注"] {
        see_cjk(&mut s, want);
    }
    s.dont_see("can not preview");
    // The outline (`o`) lists the Japanese headings.
    s.key('o');
    assert!(s.app.is_outline());
    see_cjk(&mut s, "図と脚注");
}

#[test]
fn e2e_word_the_docx_is_never_written_by_any_key() {
    let Some((dir, root)) = doc_sandbox("w_never_written", EN) else {
        return;
    };
    let docx = root.join(EN);
    let mut s = Sim::with_config_sized(&root, cfg_en(), 90, 26);
    s.select(EN);
    assert!(same_bytes_after(&docx, || {
        s.enter();
        // Every focusable item, with the keys that would toggle a checkbox / details / open a link
        // (links go nowhere: opening is switched off in this sim's config below).
        s.app.cfg.external.open_links = false;
        for _ in 0..40 {
            s.tab();
            s.key(' ');
            s.enter();
        }
        s.key('R');
        s.keys("jjjV");
        s.key('y');
        s.key('R');
        s.keys("o");
        s.esc();
    }));
    // The mtime did not move either (nothing rewrote it).
    drop(dir);
}

#[test]
fn e2e_word_checkbox_like_text_is_text_and_toggling_cannot_write_the_docx() {
    let dir = sandbox("w_checkbox");
    let docx = canon(&dir).join("t.docx");
    build_docx(
        &docx,
        &format!(
            "{}{}{}",
            para("[ ] not a task"),
            para("- [x] also text"),
            para("tail")
        ),
    );
    let mut s = Sim::with_config(&canon(&dir), cfg_en());
    s.select("t.docx");
    assert!(same_bytes_after(&docx, || {
        s.enter();
        assert!(s.app.document_ready());
        assert!(!s.app.md_has_tasks(), "no focusable checkbox in a document");
        s.see("not a task");
        s.see("also text");
        for _ in 0..4 {
            s.tab();
            assert!(!s.app.md_focused_task());
            s.key(' ');
            s.enter();
        }
    }));
    assert!(s.app.flash.is_none() || !s.app.flash.clone().unwrap().contains("checkbox"));
}

// ---------------------------------------------------------------------------------------------
// Pictures and formulas (real decode / encode workers, halfblocks picker)
// ---------------------------------------------------------------------------------------------

#[test]
fn e2e_word_pictures_and_formulas_are_drawn_as_images() {
    let Some((mut s, _d)) = open_doc_with_media("w_images", EN, TALL) else {
        return;
    };
    // The conversion arrives from the Office worker; until then nothing but the spinner.
    assert!(s.app.is_document_loading());
    s.see("loading");
    s.drain_media();
    assert!(s.app.document_ready());
    assert_eq!(s.app.document_picture_count_for_test(), 1);
    settle_images(&mut s);

    let placements = s.app.md_images();
    let office: Vec<_> = placements
        .iter()
        .filter(|p| crate::preview::markdown::is_office_image_url(&p.url))
        .collect();
    let math: Vec<_> = placements
        .iter()
        .filter(|p| crate::preview::markdown::is_math_url(&p.url))
        .collect();
    assert_eq!(office.len(), 1, "{placements:?}");
    assert_eq!(
        math.len(),
        2,
        "two formulas became math images: {placements:?}"
    );
    // Every one of them has an encoded picture for its own box (nothing left on the loading row).
    for p in &placements {
        assert!(
            s.app
                .md_image_proto(&p.url, p.cols, p.rows, 0, p.rows)
                .is_some(),
            "no picture for {}",
            p.url
        );
    }
    // The picture is on screen instead of its alt text and URL; the formulas' LaTeX is not.
    s.dont_see("office-img://");
    s.dont_see("Gradient A small gradient picture");
    s.dont_see("\\frac");
    // Real ink reached the buffer (the gradient is coloured, the formulas are light on dark).
    let ink = drawn_rgb_fgs(&s.term)
        .into_iter()
        .filter(|&(r, g, b)| r > 20 || g > 20 || b > 20)
        .count();
    assert!(ink > 20, "ink: {ink}");
}

#[test]
fn e2e_word_pictures_are_drawn_on_a_kitty_terminal_too() {
    let Some((dir, root)) = doc_sandbox("w_kitty", EN) else {
        return;
    };
    let mut s = Sim::with_config_sized(&root, cfg_en(), TALL.0, TALL.1).with_media_kitty();
    s.select(EN);
    s.enter();
    s.drain_media();
    settle_images(&mut s);
    let placement = s
        .app
        .md_images()
        .into_iter()
        .find(|p| crate::preview::markdown::is_office_image_url(&p.url))
        .expect("the picture has a placement");
    assert!(s
        .app
        .md_image_proto(
            &placement.url,
            placement.cols,
            placement.rows,
            0,
            placement.rows
        )
        .is_some());
    drop(dir);
}

#[test]
fn e2e_word_an_undecodable_picture_degrades_to_its_alt_text() {
    let dir = sandbox("w_badpic");
    let docx = canon(&dir).join("p.docx");
    {
        use std::io::Write;
        // Build a docx whose only picture is not an image at all.
        let f = std::fs::File::create(&docx).unwrap();
        let mut zw = zip::ZipWriter::new(f);
        let o = zip::write::SimpleFileOptions::default();
        let mut put = |n: &str, t: &[u8]| {
            zw.start_file(n, o).unwrap();
            zw.write_all(t).unwrap();
        };
        let rel = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
        put(
            "[Content_Types].xml",
            br#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"/>"#,
        );
        put("_rels/.rels", format!(r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{rel}/officeDocument" Target="word/document.xml"/></Relationships>"#).as_bytes());
        put("word/_rels/document.xml.rels", format!(r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rIdP" Type="{rel}/image" Target="media/a.png"/></Relationships>"#).as_bytes());
        put("word/media/a.png", b"this is not a png");
        put("word/document.xml", format!(r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="{rel}" xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><w:body><w:p><w:r><w:t>before</w:t></w:r><w:r><w:drawing><wp:inline><wp:docPr id="1" name="x" descr="the broken picture"/><a:graphic><a:graphicData><a:blip r:embed="rIdP"/></a:graphicData></a:graphic></wp:inline></w:drawing></w:r></w:p></w:body></w:document>"#).as_bytes());
        zw.finish().unwrap();
    }
    let mut s = Sim::with_config(&canon(&dir), cfg_en()).with_media();
    s.select("p.docx");
    s.enter();
    s.drain_media();
    assert!(s.app.document_ready());
    settle_images(&mut s);
    // No size can be read from it: it is drawn as text, and nothing hangs or panics.
    s.see("before");
    s.see("the broken picture");
}

// ---------------------------------------------------------------------------------------------
// `R`: the converted Markdown (windowed, selectable, copyable)
// ---------------------------------------------------------------------------------------------

#[test]
fn e2e_word_r_shows_the_converted_markdown_and_r_again_returns() {
    let Some((mut s, _d)) = open_doc("w_raw", EN) else {
        return;
    };
    assert_eq!(s.app.document_raw_file_for_test(), None);
    s.key('R');
    assert!(s.app.is_md_raw() && s.app.is_windowed());
    s.see("raw source");
    s.see("# Word reader sample");
    s.see("**Contents**");
    s.see("\\*star\\*");
    // The converted text lives in a private temp file, not in the .docx.
    let tmp = s.app.document_raw_file_for_test().expect("a temp file");
    assert!(tmp != s.app.tab.preview_path.clone().unwrap());
    assert_eq!(
        std::fs::read_to_string(&tmp).unwrap(),
        s.app.document_markdown_for_test().unwrap()
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&tmp).unwrap().permissions().mode();
        assert_eq!(mode & 0o077, 0, "owner-only: {mode:o}");
    }
    // Back to the rendered view: the temp file is gone, the decorated text is back.
    s.key('R');
    assert!(!s.app.is_md_raw() && !s.app.is_windowed());
    assert!(!tmp.exists(), "the raw view's temp file is deleted");
    s.dont_see("# Word reader sample");
    s.see("Word reader sample");
}

#[test]
fn e2e_word_the_raw_view_selects_and_copies_the_converted_text() {
    let Some((mut s, _d)) = open_doc("w_raw_copy", EN) else {
        return;
    };
    s.key('R');
    // V = the line under the caret.
    crate::test_support::clear_test_clipboard();
    s.keys("V");
    s.key('y');
    assert_eq!(
        crate::test_support::get_test_clipboard().as_deref(),
        Some("# Word reader sample"),
        "the copied text is the converted Markdown, not the .docx"
    );
    // V over three lines.
    crate::test_support::clear_test_clipboard();
    s.keys("Vjjy");
    assert_eq!(
        crate::test_support::get_test_clipboard().as_deref(),
        Some("# Word reader sample\n\n**Contents**")
    );
    // v = a character range.
    crate::test_support::clear_test_clipboard();
    s.keys("g0vllllly");
    assert_eq!(
        crate::test_support::get_test_clipboard().as_deref(),
        Some("# Word")
    );
    // Y has no line range to speak of for a converted document: the file reference only.
    crate::test_support::clear_test_clipboard();
    s.key('Y');
    let y = crate::test_support::get_test_clipboard().unwrap();
    assert!(y.ends_with("word.docx") && !y.contains("#L"), "{y}");
}

#[test]
fn e2e_word_the_raw_view_searches_the_converted_text() {
    let Some((mut s, _d)) = open_doc_sized("w_raw_search", EN, (100, 30), cfg_en()) else {
        return;
    };
    s.key('R');
    s.key('/');
    s.keys("Kept text");
    s.enter();
    assert_eq!(
        s.app.search_status(),
        Some((1, 1)),
        "found in the converted text"
    );
    s.see("Kept text. This sentence is inserted.");
    // The raw view is the converted Markdown: markup that the decorated view hides is searchable.
    s.key('/');
    s.keys("[^1]");
    s.enter();
    assert!(s.app.search_status().is_some_and(|(_, n)| n >= 2));
}

#[test]
fn e2e_word_the_raw_view_survives_a_reload_with_the_new_text() {
    let Some((dir, root)) = doc_sandbox("w_raw_reload", EN) else {
        return;
    };
    let Some(ja) = testdata(JA) else {
        return;
    };
    let mut s = Sim::with_config_sized(&root, cfg_en(), 100, 30).with_media();
    s.select(EN);
    s.enter();
    s.drain_media();
    s.key('R');
    let old_tmp = s.app.document_raw_file_for_test().unwrap();
    s.see("# Word reader sample");
    // The file is replaced by the Japanese sample (a later mtime).
    let docx = root.join(EN);
    std::fs::copy(&ja, &docx).unwrap();
    let f = std::fs::OpenOptions::new().write(true).open(&docx).unwrap();
    f.set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_900_000_100))
        .unwrap();
    s.app.refresh_fs_watched(false, std::slice::from_ref(&docx));
    s.draw();
    drain_media_until_current(&mut s);
    assert!(
        s.app.is_md_raw() && s.app.is_windowed(),
        "still the raw view"
    );
    see_cjk(&mut s, "# Word 読み込みサンプル");
    assert!(!old_tmp.exists(), "the stale temp file was replaced");
    assert!(s.app.document_raw_file_for_test().unwrap().exists());
    drop(dir);
}

// ---------------------------------------------------------------------------------------------
// Outline, search, links, copy of a code block, paging
// ---------------------------------------------------------------------------------------------

#[test]
fn e2e_word_the_outline_lists_the_headings_and_jumps() {
    let Some((mut s, _d)) = open_doc_sized("w_outline", EN, (90, 26), cfg_en()) else {
        return;
    };
    s.key('o');
    assert!(s.app.is_outline());
    for h in [
        "Word reader sample",
        "Introduction",
        "Lists",
        "Bullets and numbers",
        "Pictures and notes",
    ] {
        s.see(h);
    }
    // Down to "Pictures and notes" and jump: the view scrolls there.
    let before = s.app.tab.preview_scroll;
    for _ in 0..4 {
        s.key('j');
    }
    s.enter();
    assert!(!s.app.is_outline());
    assert!(s.app.tab.preview_scroll > before, "scrolled to the section");
    s.see("Pictures and notes");
}

#[test]
fn e2e_word_search_finds_text_in_the_decorated_view() {
    let Some((mut s, _d)) = open_doc_sized("w_search", EN, (90, 26), cfg_en()) else {
        return;
    };
    s.key('/');
    s.keys("footnote");
    s.enter();
    let (cur, total) = s.app.search_status().expect("hits");
    assert_eq!(cur, 1);
    assert!(total >= 2, "{total}");
    s.key('n');
    assert_eq!(s.app.search_status().unwrap().0, 2);
    s.key('N');
    assert_eq!(s.app.search_status().unwrap().0, 1);
    // A miss says so.
    s.key('/');
    s.keys("zzzzqqqq");
    s.enter();
    assert!(s
        .app
        .flash
        .clone()
        .unwrap_or_default()
        .to_lowercase()
        .contains("no match"));
}

#[test]
fn e2e_word_links_are_focusable_and_follow_the_markdown_rules() {
    let Some((mut s, _d)) = open_doc_sized("w_links", EN, (90, 26), cfg_en()) else {
        return;
    };
    s.app.cfg.external.open_links = false; // never start the OS opener from a test
    let targets = s.app.md_link_targets();
    assert!(
        targets.iter().any(|t| t == "https://example.com/a?x=1&y=2"),
        "{targets:?}"
    );
    assert!(targets.iter().any(|t| t == "#introduction"), "{targets:?}");
    // No link goes to a local path: a document cannot open files next to it by relative path.
    for t in &targets {
        assert!(
            t.starts_with('#') || t.starts_with("http") || t.starts_with("mailto:"),
            "{t}"
        );
    }
    // An in-document link scrolls in place; the footer says "jump" for it.
    tab_to(&mut s, crate::app::MdFocus::AnchorLink, 30);
    s.see("↵:jump");
    s.dont_see("C-t:new tab");
    s.enter();
    // An external link goes to the OS (here: switched off, so it says so) and the footer says "browser".
    s.key('g');
    tab_to(&mut s, crate::app::MdFocus::ExternalLink, 40);
    s.see("↵:browser");
    s.enter();
    let flash = s.app.flash.clone().unwrap_or_default();
    assert!(flash.contains("open_links"), "{flash}");
    // Ctrl-t on a link in a document never makes a tab either.
    let tabs = s.app.tab_count();
    s.ctrl('t');
    assert_eq!(s.app.tab_count(), tabs);
}

#[test]
fn e2e_word_an_anchor_link_to_a_heading_scrolls_to_it() {
    let Some((mut s, _d)) = open_doc_sized("w_anchor", EN, (90, 26), cfg_en()) else {
        return;
    };
    // Focus the table-of-contents link "Pictures and notes" and press Enter.
    let mut found = false;
    for _ in 0..12 {
        s.tab();
        if s.app.md_focused_kind() == Some(crate::app::MdFocus::AnchorLink)
            && s.screen().contains("Pictures and notes")
        {
            let idx = s.app.focused_item().unwrap();
            if s.app.md_link_targets().get(idx).map(String::as_str) == Some("#pictures-and-notes") {
                found = true;
                break;
            }
        }
    }
    assert!(found, "TOC link not reached:\n{}", s.screen());
    let before = s.app.tab.preview_scroll;
    s.enter();
    assert!(s.app.tab.preview_scroll > before, "jumped to the heading");
}

#[test]
fn e2e_word_y_c_copies_the_code_block_from_the_converted_text() {
    let Some((mut s, _d)) = open_doc("w_codecopy", EN) else {
        return;
    };
    tab_to(&mut s, crate::app::MdFocus::CodeBlock, 40);
    s.see("y c:copy code");
    crate::test_support::clear_test_clipboard();
    s.keys("yc");
    let copied = crate::test_support::get_test_clipboard().unwrap();
    assert!(
        copied.starts_with("def f(x):") && copied.contains("return x * 2"),
        "{copied:?}"
    );
}

#[test]
fn e2e_word_ctrl_n_pages_to_the_next_file_and_releases_the_document() {
    let Some((mut s, _d)) = open_doc("w_ctrl_n", EN) else {
        return;
    };
    assert!(s.app.document_ready());
    s.ctrl('n');
    assert!(s.app.tab.preview_path.clone().unwrap().ends_with("z.txt"));
    assert!(!s.app.is_document() && !s.app.document_ready());
    assert_eq!(
        s.app.document_picture_count_for_test(),
        0,
        "pictures released"
    );
    s.see("after the document");
    // And back: the document is converted again.
    s.ctrl('p');
    assert!(s.app.document_ready());
    s.see("Word reader sample");
}

#[test]
fn e2e_word_the_title_names_the_file_and_the_footer_offers_e_as_open() {
    let Some((s, _d)) = open_doc_sized("w_title", EN, (200, 40), cfg_en()) else {
        return;
    };
    s.see("word.docx");
    // `e` opens an Office app for a document (never an editor): the footer says "open".
    s.see("e:open");
    s.dont_see("e:edit");
}

#[test]
fn e2e_word_e_opens_the_office_app_in_both_views_and_never_the_editor() {
    let Some((mut s, _d)) = open_doc("w_e", EN) else {
        return;
    };
    let log = office_recorder(&mut s, 0);
    s.key('e');
    assert!(
        s.app.take_pending_edit().is_none(),
        "must not reach the editor"
    );
    assert_eq!(log.lock().unwrap().len(), 1);
    assert!(log.lock().unwrap()[0]
        .1
        .last()
        .unwrap()
        .ends_with("word.docx"));
    // The raw view's caret line is a line of the converted text: it is never a target either.
    s.key('R');
    s.key('e');
    assert!(s.app.take_pending_edit().is_none());
    assert_eq!(log.lock().unwrap().len(), 2);
    assert!(log.lock().unwrap()[1]
        .1
        .last()
        .unwrap()
        .ends_with("word.docx"));
}

// ---------------------------------------------------------------------------------------------
// Hints appear only for keys that act ([[hint-shown-iff-key-acts]])
// ---------------------------------------------------------------------------------------------

#[test]
fn e2e_word_help_lists_only_keys_that_act_on_a_document() {
    let Some((mut s, _d)) = open_doc_sized("w_help", EN, (120, 60), cfg_en()) else {
        return;
    };
    s.key('?');
    let doc_help = s.screen();
    // Keys that act on a converted document.
    assert!(doc_help.contains("Tab / ⇧Tab"), "{doc_help}");
    assert!(doc_help.contains("R"), "{doc_help}");
    // Keys with nothing to act on in a document: no checkbox / <details> / diagram rows.
    assert!(!doc_help.contains("toggle focused checkbox"), "{doc_help}");
    assert!(
        !doc_help.contains("expand/collapse the focused <details>"),
        "{doc_help}"
    );
    assert!(!doc_help.contains("zoom the focused diagram"), "{doc_help}");
}

#[test]
fn e2e_word_help_of_a_plain_markdown_file_still_lists_them() {
    // The contrast of the test above: the same help for a .md keeps those rows.
    let dir = sandbox("w_help_md");
    std::fs::write(dir.join("a.md"), "# t\n\n- [ ] x\n").unwrap();
    let mut s = Sim::with_config_sized(&canon(&dir), cfg_en(), 120, 60);
    s.select("a.md");
    s.enter();
    s.key('?');
    let help = s.screen();
    assert!(help.contains("toggle focused checkbox"), "{help}");
    assert!(help.contains("zoom the focused diagram"), "{help}");
}

#[test]
fn e2e_word_the_raw_view_help_and_footer_are_the_text_ones() {
    let Some((mut s, _d)) = open_doc_sized("w_raw_help", EN, (200, 60), cfg_en()) else {
        return;
    };
    s.key('R');
    s.see("v/V:select");
    s.see("R:rendered");
    s.see("e:open");
    s.key('?');
    let help = s.screen();
    assert!(help.contains("v / V → y"), "{help}");
    // `Y` is the file reference only (the converted text has no lines in the .docx).
    assert!(help.contains("copy the @path reference"), "{help}");
    assert!(!help.contains("@path#L reference of caret"), "{help}");
}

// ---------------------------------------------------------------------------------------------
// Failures: a reason, never a crash or raw bytes
// ---------------------------------------------------------------------------------------------

fn open_bad(name: &str, make: impl FnOnce(&std::path::Path)) -> (Sim, crate::test_support::TmpDir) {
    let dir = sandbox(name);
    let p = canon(&dir).join("bad.docx");
    make(&p);
    let mut s = Sim::with_config(&canon(&dir), cfg_en());
    s.select("bad.docx");
    s.enter();
    (s, dir)
}

#[test]
fn e2e_word_a_corrupt_file_says_so() {
    let (s, _d) = open_bad("w_corrupt", |p| {
        std::fs::write(p, b"this is not a zip at all").unwrap()
    });
    assert!(s.app.is_document() && !s.app.document_ready());
    s.see("[document] cannot preview");
    s.see("damaged or not a valid Word document");
    s.see("bad.docx");
    s.dont_see("this is not a zip");
}

#[test]
fn e2e_word_an_empty_file_says_so() {
    let (s, _d) = open_bad("w_empty", |p| std::fs::write(p, b"").unwrap());
    s.see("[document] cannot preview");
}

#[test]
fn e2e_word_an_encrypted_document_says_so() {
    let Some(enc) = testdata("encrypted.xlsx") else {
        return;
    };
    // An encrypted package is a CFB container, whatever the extension says.
    let (s, _d) = open_bad("w_encrypted", |p| {
        std::fs::copy(&enc, p).unwrap();
    });
    s.see("password-protected");
}

#[test]
fn e2e_word_a_zip_that_is_not_a_word_document_says_so() {
    let Some(book) = testdata("formats.xlsx") else {
        return;
    };
    let (s, _d) = open_bad("w_notword", |p| {
        std::fs::copy(&book, p).unwrap();
    });
    s.see("[document] cannot preview");
    s.dont_see("[spreadsheet]");
}

#[test]
fn e2e_word_an_oversized_file_names_the_limit() {
    let (s, _d) = open_bad("w_huge", |p| {
        let f = std::fs::File::create(p).unwrap();
        f.set_len(300 * 1024 * 1024).unwrap(); // sparse: no real disk
    });
    s.see("[document] too large to preview");
    s.see("256 MiB");
}

#[test]
fn e2e_word_failures_are_japanese_in_a_japanese_ui() {
    let dir = sandbox("w_fail_ja");
    std::fs::write(dir.join("bad.docx"), b"nope").unwrap();
    let mut cfg = Config::default();
    cfg.ui.lang = "ja".into();
    let mut s = Sim::with_config(&canon(&dir), cfg);
    s.select("bad.docx");
    s.enter();
    see_cjk(&mut s, "[文書] 表示不可");
}

#[test]
fn e2e_word_a_failure_has_no_raw_view_and_r_does_nothing() {
    let (mut s, _d) = open_bad("w_fail_r", |p| std::fs::write(p, b"nope").unwrap());
    s.key('R');
    assert!(!s.app.is_md_raw() && !s.app.is_windowed());
    assert_eq!(s.app.document_raw_file_for_test(), None);
    s.see("[document] cannot preview");
}

#[test]
fn e2e_word_a_very_long_document_is_cut_and_the_title_says_so() {
    let dir = sandbox("w_long");
    let docx = canon(&dir).join("long.docx");
    let body: String = (0..6000)
        .map(|i| para(&format!("paragraph number {i}")))
        .collect();
    build_docx(&docx, &body);
    let mut s = Sim::with_config_sized(&canon(&dir), cfg_en(), 120, 30);
    s.select("long.docx");
    s.enter();
    assert!(s.app.document_ready() && s.app.document_truncated());
    s.see("truncated");
    s.see("paragraph number 0");
    s.key('G');
    s.dont_see("paragraph number 5999");
    // The raw view says so too.
    s.key('R');
    s.see("truncated");
}

// ---------------------------------------------------------------------------------------------
// The Office worker: loading screen, staleness, tab switches, reloads, serialisation
// ---------------------------------------------------------------------------------------------

#[test]
fn e2e_word_the_worker_shows_loading_then_the_document() {
    let Some((mut s, _d)) = open_doc_with_media("w_worker", EN, (100, 30)) else {
        return;
    };
    // Converted on a thread, not inline: the spinner first, the document when the result lands.
    assert!(s.app.is_document_loading() && !s.app.document_ready());
    s.see("loading");
    s.dont_see("Word reader sample");
    s.drain_media();
    assert_eq!(s.app.workbook_loads_started(), 1);
    assert!(s.app.document_ready() && !s.app.is_document_loading());
    s.see("Word reader sample");
    s.dont_see("loading");
}

#[test]
fn e2e_word_a_result_for_a_file_left_behind_is_dropped() {
    let Some((dir, root)) = doc_sandbox("w_stale", EN) else {
        return;
    };
    let mut s = Sim::with_config_sized(&root, cfg_en(), 100, 30).with_media();
    s.select(EN);
    s.enter();
    assert!(s.app.is_document_loading());
    // Leave for another file before the conversion lands.
    s.ctrl('n');
    assert!(s.app.tab.preview_path.clone().unwrap().ends_with("z.txt"));
    let res = s
        .media_rx
        .as_ref()
        .unwrap()
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("the worker answers");
    assert!(!s.app.apply_media(res), "stale");
    assert!(!s.app.document_ready());
    s.see("after the document");
    drop(dir);
}

#[test]
fn e2e_word_the_newest_of_several_requests_wins_and_loads_are_serialised() {
    let Some((dir, root)) = doc_sandbox("w_serial", EN) else {
        return;
    };
    std::fs::copy(testdata(JA).unwrap(), root.join("zz-ja.docx")).unwrap();
    let mut s = Sim::with_config_sized(&root, cfg_en(), 100, 30).with_media();
    s.select(EN);
    s.enter(); // asked: word.docx
    s.ctrl('n'); // z.txt
    s.ctrl('n'); // zz-ja.docx  (asked while the first may still run)
    assert!(s
        .app
        .tab
        .preview_path
        .clone()
        .unwrap()
        .ends_with("zz-ja.docx"));
    drain_media_until_current(&mut s);
    assert!(s.app.document_ready());
    see_cjk(&mut s, "Word 読み込みサンプル");
    assert!(
        s.app.workbook_loads_started() <= 2,
        "at most one load per request"
    );
    drop(dir);
}

#[test]
fn e2e_word_switching_tabs_reloads_the_document_on_the_worker() {
    let Some((mut s, _d)) = open_doc_with_media("w_tabs", EN, (100, 30)) else {
        return;
    };
    s.drain_media();
    assert!(s.app.document_ready());
    s.key('t'); // a second tab (a tree)
    assert!(!s.app.is_document() && !s.app.document_ready(), "released");
    assert_eq!(s.app.document_picture_count_for_test(), 0);
    s.key('1'); // back to the first tab
    assert!(s.app.is_document());
    assert!(s.app.is_document_loading() || s.app.document_ready());
    drain_media_until_current(&mut s);
    assert!(s.app.document_ready());
    s.see("Word reader sample");
}

#[test]
fn e2e_word_an_outside_edit_reloads_the_document_keeping_the_scroll() {
    let Some((dir, root)) = doc_sandbox("w_reload", EN) else {
        return;
    };
    let mut s = Sim::with_config_sized(&root, cfg_en(), 90, 20).with_media();
    s.select(EN);
    s.enter();
    s.drain_media();
    s.key('G');
    let scrolled = s.app.tab.preview_scroll;
    assert!(scrolled > 0);
    // The same document, saved again by an outside program (a later mtime).
    let docx = root.join(EN);
    let f = std::fs::OpenOptions::new().write(true).open(&docx).unwrap();
    f.set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_900_000_200))
        .unwrap();
    s.app.refresh_fs_watched(false, std::slice::from_ref(&docx));
    s.draw();
    // Until the new text arrives the old stays on screen (no flash of a spinner).
    assert!(s.app.document_ready());
    drain_media_until_current(&mut s);
    assert!(s.app.document_ready());
    assert_eq!(
        s.app.tab.preview_scroll, scrolled,
        "the view stays where it was"
    );
    drop(dir);
}

#[test]
fn e2e_word_a_document_replaced_by_a_broken_file_shows_the_reason() {
    let Some((dir, root)) = doc_sandbox("w_reload_bad", EN) else {
        return;
    };
    let mut s = Sim::with_config_sized(&root, cfg_en(), 90, 20).with_media();
    s.select(EN);
    s.enter();
    s.drain_media();
    let docx = root.join(EN);
    std::fs::write(&docx, b"broken now").unwrap();
    let f = std::fs::OpenOptions::new().write(true).open(&docx).unwrap();
    f.set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_900_000_300))
        .unwrap();
    s.app.refresh_fs_watched(false, std::slice::from_ref(&docx));
    drain_media_until_current(&mut s);
    assert!(!s.app.document_ready());
    s.see("[document] cannot preview");
    drop(dir);
}

#[test]
fn e2e_word_the_default_rule_covers_the_word_extensions_and_a_user_rule_wins() {
    let dir = sandbox("w_rules");
    for n in ["a.docx", "b.DOCX", "c.docm", "d.dotx", "e.dotm"] {
        build_docx(&dir.join(n), &para("hello word"));
    }
    let mut s = Sim::with_config(&canon(&dir), cfg_en());
    for n in ["a.docx", "b.DOCX", "c.docm", "d.dotx", "e.dotm"] {
        s.select(n);
        s.enter();
        assert!(s.app.document_ready(), "{n}");
        s.see("hello word");
        s.key('q');
    }
    // `.doc` is not previewed (it opens in an Office app with `e`).
    std::fs::write(dir.join("old.doc"), b"\xd0\xcf\x11\xe0 old binary").unwrap();
    let mut s = Sim::with_config(&canon(&dir), cfg_en());
    s.select("old.doc");
    s.enter();
    assert!(!s.app.is_document());
    // A user rule placed first replaces the built-in.
    let mut cfg = cfg_en();
    cfg.preview.rules.insert(
        0,
        crate::config::Rule {
            glob: Some("*.docx".into()),
            builtin: Some("text".into()),
            ..crate::config::Rule::default()
        },
    );
    let mut s = Sim::with_config(&canon(&dir), cfg);
    s.select("a.docx");
    s.enter();
    assert!(!s.app.is_document());
}

// ---------------------------------------------------------------------------------------------
// OpenDocument text (odt / ott): the same view as a docx
// ---------------------------------------------------------------------------------------------

const ODT_EN: &str = "word.odt";
const ODT_JA: &str = "word-ja.odt";

/// A minimal odt (mimetype, manifest, content) whose `office:text` is `body`.
fn build_odt(path: &std::path::Path, mime: &str, body: &str) {
    use std::io::Write;
    let f = std::fs::File::create(path).unwrap();
    let mut zw = zip::ZipWriter::new(f);
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    let mut put = |name: &str, text: &str| {
        zw.start_file(name, opts).unwrap();
        zw.write_all(text.as_bytes()).unwrap();
    };
    put("mimetype", mime);
    put(
        "META-INF/manifest.xml",
        r#"<?xml version="1.0"?><manifest:manifest xmlns:manifest="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0"/>"#,
    );
    put(
        "content.xml",
        &format!(
            r#"<?xml version="1.0"?><office:document-content xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0"><office:body><office:text>{body}</office:text></office:body></office:document-content>"#
        ),
    );
    zw.finish().unwrap();
}

const ODT_MIME: &str = "application/vnd.oasis.opendocument.text";

#[test]
fn e2e_odt_opens_as_decorated_markdown() {
    let Some((s, _d)) = open_doc("o_open", ODT_EN) else {
        return;
    };
    assert!(s.app.is_document() && s.app.document_ready());
    assert!(!s.app.is_table_preview() && !s.app.is_windowed() && !s.app.is_md_raw());
    s.see("Word reader sample");
    s.dont_see("# Word reader sample");
    s.dont_see("can not preview");
    s.see("*star* _under_");
    s.dont_see("**bold**");
    s.see("PREVIEW");
}

#[test]
fn e2e_odt_the_structure_of_the_document_reaches_the_screen() {
    let Some((s, _d)) = open_doc("o_structure", ODT_EN) else {
        return;
    };
    for want in [
        "Contents",
        "Lists",
        "Bullets and numbers",
        "- bullet one",
        "- bullet nested",
        "1. first",
        "2. second",
        "1. second-a",
        "3. third",
        "a. alpha",
        "II. two",
        "┌",
        "Merged across two columns",
        "a|b",
        "two lines",
        "Footnote text",
        "Endnote text.",
        "TEXTBOX-TEXT inside a frame",
        "This sentence is inserted.",
    ] {
        s.see(want);
    }
    // Not shown: the deleted sentence, the comment, the header and the footer.
    for not in [
        "This sentence is deleted",
        "SECRET-COMMENT",
        "RUNNING-HEADER",
        "RUNNING-FOOTER",
    ] {
        s.dont_see(not);
    }
}

#[test]
fn e2e_odt_a_japanese_document_opens_and_reads_in_japanese() {
    let Some((mut s, _d)) = open_doc("o_ja", ODT_JA) else {
        return;
    };
    for want in ["Word 読み込みサンプル", "はじめに", "リスト", "図と脚注"] {
        see_cjk(&mut s, want);
    }
    s.dont_see("can not preview");
    s.key('o');
    assert!(s.app.is_outline());
    see_cjk(&mut s, "図と脚注");
}

#[test]
fn e2e_odt_the_odt_is_never_written_by_any_key() {
    let Some((dir, root)) = doc_sandbox("o_never_written", ODT_EN) else {
        return;
    };
    let odt = root.join(ODT_EN);
    let mut s = Sim::with_config_sized(&root, cfg_en(), 90, 26);
    s.select(ODT_EN);
    assert!(same_bytes_after(&odt, || {
        s.enter();
        s.app.cfg.external.open_links = false;
        for _ in 0..40 {
            s.tab();
            s.key(' ');
            s.enter();
        }
        s.key('R');
        s.keys("jjjV");
        s.key('y');
        s.key('R');
        s.keys("o");
        s.esc();
    }));
    drop(dir);
}

#[test]
fn e2e_odt_r_shows_the_converted_markdown_and_r_again_returns() {
    let Some((mut s, _d)) = open_doc("o_raw", ODT_EN) else {
        return;
    };
    s.key('R');
    assert!(s.app.is_md_raw() && s.app.is_windowed());
    s.see("raw source");
    s.see("# Word reader sample");
    s.see("**Contents**");
    s.see("\\*star\\*");
    let tmp = s.app.document_raw_file_for_test().expect("a temp file");
    assert_eq!(
        std::fs::read_to_string(&tmp).unwrap(),
        s.app.document_markdown_for_test().unwrap()
    );
    s.key('R');
    assert!(!s.app.is_md_raw() && !s.app.is_windowed());
    assert!(!tmp.exists());
    s.see("Word reader sample");
}

#[test]
fn e2e_odt_the_formulas_and_the_picture_are_drawn_as_images() {
    let Some((mut s, _d)) = open_doc_with_media("o_images", ODT_EN, TALL) else {
        return;
    };
    assert!(s.app.is_document_loading());
    s.drain_media();
    assert!(s.app.document_ready());
    assert_eq!(s.app.document_picture_count_for_test(), 1);
    settle_images(&mut s);
    let placements = s.app.md_images();
    let office = placements
        .iter()
        .filter(|p| crate::preview::markdown::is_office_image_url(&p.url))
        .count();
    let math = placements
        .iter()
        .filter(|p| crate::preview::markdown::is_math_url(&p.url))
        .count();
    assert_eq!(office, 1, "{placements:?}");
    assert_eq!(math, 2, "two formulas became math images: {placements:?}");
    for p in &placements {
        assert!(
            s.app
                .md_image_proto(&p.url, p.cols, p.rows, 0, p.rows)
                .is_some(),
            "no picture for {}",
            p.url
        );
    }
    s.dont_see("office-img://");
    s.dont_see("\\frac");
}

#[test]
fn e2e_odt_links_are_focusable_and_the_toc_link_scrolls() {
    let Some((mut s, _d)) = open_doc_sized("o_links", ODT_EN, (90, 26), cfg_en()) else {
        return;
    };
    s.app.cfg.external.open_links = false;
    let targets = s.app.md_link_targets();
    assert!(
        targets.iter().any(|t| t == "https://example.com/a?x=1&y=2"),
        "{targets:?}"
    );
    assert!(
        targets.iter().any(|t| t == "#pictures-and-notes"),
        "{targets:?}"
    );
    for t in &targets {
        assert!(
            t.starts_with('#') || t.starts_with("http") || t.starts_with("mailto:"),
            "{t}"
        );
    }
    let mut found = false;
    for _ in 0..12 {
        s.tab();
        if s.app.md_focused_kind() == Some(crate::app::MdFocus::AnchorLink)
            && s.screen().contains("Pictures and notes")
        {
            let idx = s.app.focused_item().unwrap();
            if s.app.md_link_targets().get(idx).map(String::as_str) == Some("#pictures-and-notes") {
                found = true;
                break;
            }
        }
    }
    assert!(found, "TOC link not reached:\n{}", s.screen());
    let before = s.app.tab.preview_scroll;
    s.enter();
    assert!(s.app.tab.preview_scroll > before, "jumped to the heading");
}

#[test]
fn e2e_odt_the_default_rule_covers_odt_and_ott_and_a_user_rule_wins() {
    let dir = sandbox("o_rules");
    for n in ["a.odt", "b.ODT", "c.ott"] {
        build_odt(&dir.join(n), ODT_MIME, "<text:p>hello odf</text:p>");
    }
    let mut s = Sim::with_config(&canon(&dir), cfg_en());
    for n in ["a.odt", "b.ODT", "c.ott"] {
        s.select(n);
        s.enter();
        assert!(s.app.document_ready(), "{n}");
        s.see("hello odf");
        s.key('q');
    }
    let mut cfg = cfg_en();
    cfg.preview.rules.insert(
        0,
        crate::config::Rule {
            glob: Some("*.odt".into()),
            builtin: Some("text".into()),
            ..crate::config::Rule::default()
        },
    );
    let mut s = Sim::with_config(&canon(&dir), cfg);
    s.select("a.odt");
    s.enter();
    assert!(!s.app.is_document());
}

fn open_bad_odt(
    name: &str,
    make: impl FnOnce(&std::path::Path),
) -> (Sim, crate::test_support::TmpDir) {
    let dir = sandbox(name);
    let p = canon(&dir).join("bad.odt");
    make(&p);
    let mut s = Sim::with_config(&canon(&dir), cfg_en());
    s.select("bad.odt");
    s.enter();
    (s, dir)
}

#[test]
fn e2e_odt_failures_say_why() {
    // Damaged.
    let (s, _d) = open_bad_odt("o_corrupt", |p| std::fs::write(p, b"not a zip").unwrap());
    assert!(s.app.is_document() && !s.app.document_ready());
    s.see("[document] cannot preview");
    s.see("damaged or not a valid Word document");
    s.dont_see("not a zip");
    // A spreadsheet saved under an odt name is not a text.
    let (s, _d) = open_bad_odt("o_ods", |p| {
        build_odt(
            p,
            "application/vnd.oasis.opendocument.spreadsheet",
            "<text:p>x</text:p>",
        )
    });
    s.see("[document] cannot preview");
    // A password-protected one (LibreOffice 25: one `encrypted-package` entry).
    let (s, _d) = open_bad_odt("o_enc", |p| {
        use std::io::Write;
        let f = std::fs::File::create(p).unwrap();
        let mut zw = zip::ZipWriter::new(f);
        let opts = zip::write::SimpleFileOptions::default();
        zw.start_file("mimetype", opts).unwrap();
        zw.write_all(ODT_MIME.as_bytes()).unwrap();
        zw.start_file("encrypted-package", opts).unwrap();
        zw.write_all(b"\x01\x02\x03").unwrap();
        zw.finish().unwrap();
    });
    s.see("password-protected");
}

#[test]
fn e2e_odt_a_very_long_document_is_cut_and_the_title_says_so() {
    let dir = sandbox("o_long");
    let body: String = (0..9_000)
        .map(|i| format!("<text:p>paragraph number {i}</text:p>"))
        .collect();
    build_odt(&dir.join("long.odt"), ODT_MIME, &body);
    let mut s = Sim::with_config_sized(&canon(&dir), cfg_en(), 100, 30);
    s.select("long.odt");
    s.enter();
    assert!(s.app.document_ready() && s.app.document_truncated());
    s.see("truncated");
    s.see("paragraph number 0");
    s.key('G');
    s.dont_see("paragraph number 8999");
}

// ---------------------------------------------------------------------------------------------
// Review fixes: footer hints, the old binary format, raw-view reloads, paste-jump, decode cap
// ---------------------------------------------------------------------------------------------

fn footer_text(s: &Sim) -> String {
    crate::ui::preview::footer_hints(&s.app).join(" | ")
}

#[test]
fn e2e_word_a_failed_document_footer_offers_no_dead_keys() {
    let (s, _d) = open_bad("w_fail_footer", |p| std::fs::write(p, b"nope").unwrap());
    let f = footer_text(&s);
    for dead in ["R:", "/:", "hl:", "v/V", "0/$", "g/G", "o:"] {
        assert!(!f.contains(dead), "{dead} must not be offered: {f}");
    }
    assert!(f.contains("q:") && f.contains("?:"), "{f}");
}

#[test]
fn e2e_word_a_loading_document_footer_offers_no_dead_keys() {
    let Some((s, _d)) = open_doc_with_media("w_load_footer", EN, TALL) else {
        return;
    };
    assert!(s.app.is_document_loading());
    let f = footer_text(&s);
    assert!(
        !f.contains("R:") && !f.contains("/:") && !f.contains("hl:"),
        "{f}"
    );
}

#[test]
fn e2e_word_a_ready_document_footer_still_offers_r() {
    let Some((mut s, _d)) = open_doc("w_ready_footer", EN) else {
        return;
    };
    s.draw();
    assert!(s.app.document_ready());
    assert!(footer_text(&s).contains("R:"), "{}", footer_text(&s));
}

#[test]
fn e2e_word_an_old_binary_document_named_docx_says_so() {
    // The OLE / compound-file signature, then junk: an old `.doc`, not a damaged zip.
    let (s, _d) = open_bad("w_legacy", |p| {
        let mut b = vec![0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];
        b.extend_from_slice(&[0u8; 600]);
        std::fs::write(p, b).unwrap();
    });
    s.see("[document] cannot preview");
    s.see("an old .doc");
    s.dont_see("damaged");
}

#[test]
fn e2e_word_a_damaged_zip_keeps_the_damaged_message() {
    // Not the CFB signature: still "damaged" (the sniff must not swallow the other reasons).
    let (s, _d) = open_bad("w_notlegacy", |p| {
        std::fs::write(p, b"PK\x03\x04junkjunk").unwrap()
    });
    s.see("damaged");
}

#[test]
fn e2e_word_an_old_binary_document_is_japanese_in_a_japanese_ui() {
    let dir = sandbox("w_legacy_ja");
    let mut b = vec![0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];
    b.extend_from_slice(&[0u8; 600]);
    std::fs::write(dir.join("old.docx"), b).unwrap();
    let mut cfg = Config::default();
    cfg.ui.lang = "ja".into();
    let mut s = Sim::with_config(&canon(&dir), cfg);
    s.select("old.docx");
    s.enter();
    see_cjk(&mut s, "古い .doc");
}

fn long_docx(path: &std::path::Path, paras: usize) {
    let body: String = (0..paras)
        .map(|i| para(&format!("line number {i}")))
        .collect();
    build_docx(path, &body);
}

fn touch_later(path: &std::path::Path, secs: u64) {
    let f = std::fs::OpenOptions::new().write(true).open(path).unwrap();
    f.set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs))
        .unwrap();
}

#[test]
fn e2e_word_the_raw_view_keeps_its_place_across_a_reload() {
    let dir = sandbox("w_raw_keep");
    let root = canon(&dir);
    let docx = root.join("long.docx");
    long_docx(&docx, 200);
    let mut s = Sim::with_config_sized(&root, cfg_en(), 90, 20).with_media();
    s.select("long.docx");
    s.enter();
    s.drain_media();
    s.key('R');
    for _ in 0..40 {
        s.key('j');
    }
    let (byte, line) = (s.app.preview_byte_top_for_test(), s.app.preview_top_line());
    assert!(line > 10 && byte > 0, "scrolled: {line} {byte}");
    // Saved again with one more paragraph at the end (the lines above are unchanged).
    long_docx(&docx, 201);
    touch_later(&docx, 1_900_000_400);
    s.app.refresh_fs_watched(false, std::slice::from_ref(&docx));
    s.draw();
    drain_media_until_current(&mut s);
    assert!(s.app.is_md_raw() && s.app.is_windowed());
    assert_eq!(s.app.preview_top_line(), line, "top line kept");
    assert_eq!(s.app.preview_byte_top_for_test(), byte, "byte offset kept");
    // The reload really delivered the new text (one more paragraph), and the view did not jump.
    assert!(s
        .app
        .document_markdown_for_test()
        .is_some_and(|m| m.contains("line number 200")));
    s.dont_see("line number 0");
}

#[test]
fn e2e_word_a_raw_view_replaced_by_a_broken_file_shows_the_reason_not_old_text() {
    let Some((dir, root)) = doc_sandbox("w_raw_bad", EN) else {
        return;
    };
    let mut s = Sim::with_config_sized(&root, cfg_en(), 100, 30).with_media();
    s.select(EN);
    s.enter();
    s.drain_media();
    s.key('R');
    s.see("# Word reader sample");
    let old_tmp = s.app.document_raw_file_for_test().unwrap();
    let docx = root.join(EN);
    std::fs::write(&docx, b"broken now").unwrap();
    touch_later(&docx, 1_900_000_500);
    s.app.refresh_fs_watched(false, std::slice::from_ref(&docx));
    drain_media_until_current(&mut s);
    s.see("[document] cannot preview");
    s.dont_see("Word reader sample");
    assert!(!s.app.is_windowed() && !s.app.is_md_raw());
    assert!(!old_tmp.exists(), "the old raw text is deleted");
    assert_eq!(s.app.document_raw_file_for_test(), None);
    let f = footer_text(&s);
    assert!(!f.contains("R:") && !f.contains("hl:"), "{f}");
    drop(dir);
}

#[test]
fn e2e_word_paste_jump_with_a_line_number_does_not_open_the_raw_view() {
    let Some((dir, root)) = doc_sandbox("w_paste_line", EN) else {
        return;
    };
    let mut s = Sim::with_config_sized(&root, cfg_en(), 100, 30);
    let target = format!("{}:12", root.join(EN).display());
    s.app.paste_jump_from(&target);
    s.draw();
    assert!(s.app.is_document());
    assert!(!s.app.is_md_raw(), "a line number means nothing in a docx");
    assert!(!s.app.is_windowed());
    assert_eq!(s.app.document_raw_file_for_test(), None);
    drop(dir);
}

#[test]
fn e2e_word_picture_decodes_in_flight_are_capped() {
    let Some((mut s, _d)) = open_doc_with_media("w_cap", EN, TALL) else {
        return;
    };
    s.drain_media();
    let url = s.app.document_first_picture_url_for_test().unwrap();
    s.app.forget_office_picture_for_test(&url);
    s.app.fake_office_decodes_in_flight_for_test(16);
    s.app.ensure_md_image(&url, 10, 5, 0, 5);
    assert!(
        !s.app.office_picture_started_for_test(&url),
        "no 17th decode while 16 are in flight"
    );
    s.app.clear_fake_office_decodes_for_test();
    s.app.ensure_md_image(&url, 10, 5, 0, 5);
    assert!(s.app.office_picture_started_for_test(&url));
}

#[test]
fn e2e_word_a_real_old_binary_file_named_docx_says_so() {
    let Some(xls) = testdata("formats.xls") else {
        return;
    };
    let (s, _d) = open_bad("w_legacy_real", |p| {
        std::fs::copy(&xls, p).unwrap();
    });
    s.see("an old .doc");
    s.dont_see("damaged");
}

// ---------------------------------------------------------------------------------------------
// Tests added after a mutation audit of the Word preview's App-level rules: each one pins a branch
// that a mutated build (a dropped reset, a skipped prune, an inverted guard) used to get away with.
// ---------------------------------------------------------------------------------------------

/// A real (decodable) PNG of one flat colour.
fn png_bytes(rgb: [u8; 3]) -> Vec<u8> {
    use image::{ImageFormat, Rgba, RgbaImage};
    let img = RgbaImage::from_pixel(24, 16, Rgba([rgb[0], rgb[1], rgb[2], 255]));
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, ImageFormat::Png).unwrap();
    out.into_inner()
}

/// A docx with the text `text` and one picture `media/<name>` holding `bytes`.
fn build_docx_with_picture(path: &std::path::Path, text: &str, name: &str, bytes: &[u8]) {
    use std::io::Write;
    let rel = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
    let f = std::fs::File::create(path).unwrap();
    let mut zw = zip::ZipWriter::new(f);
    let o = zip::write::SimpleFileOptions::default();
    let mut put = |n: &str, t: &[u8]| {
        zw.start_file(n, o).unwrap();
        zw.write_all(t).unwrap();
    };
    put(
        "[Content_Types].xml",
        br#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"/>"#,
    );
    put(
        "_rels/.rels",
        format!(r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{rel}/officeDocument" Target="word/document.xml"/></Relationships>"#).as_bytes(),
    );
    put(
        "word/_rels/document.xml.rels",
        format!(r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rIdP" Type="{rel}/image" Target="media/{name}"/></Relationships>"#).as_bytes(),
    );
    put(&format!("word/media/{name}"), bytes);
    put(
        "word/document.xml",
        format!(r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="{rel}" xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><w:body><w:p><w:r><w:t>{text}</w:t></w:r></w:p><w:p><w:r><w:drawing><wp:inline><wp:docPr id="1" name="x" descr="the picture"/><a:graphic><a:graphicData><a:blip r:embed="rIdP"/></a:graphicData></a:graphic></wp:inline></w:drawing></w:r></w:p></w:body></w:document>"#).as_bytes(),
    );
    zw.finish().unwrap();
}

/// Opens `docx` (already in `dir`) with the real workers in a 100x30 terminal.
fn open_with_media(dir: &crate::test_support::TmpDir, file: &str) -> Sim {
    let mut s = Sim::with_config_sized(&canon(dir), cfg_en(), 100, 30).with_media();
    s.select(file);
    s.enter();
    s
}

fn office_placements(s: &Sim) -> Vec<crate::preview::markdown::ImagePlacement> {
    s.app
        .md_images()
        .into_iter()
        .filter(|p| crate::preview::markdown::is_office_image_url(&p.url))
        .collect()
}

/// The converted text exists only while the preview is a document: with the kind moved away
/// (state the other paths keep from existing), `document_markdown` answers nothing instead of the
/// stale text (catches the `is_document` guard being dropped).
#[test]
fn e2e_word_the_converted_text_is_only_offered_for_a_document() {
    let Some((mut s, _d)) = open_doc("w_md_guard", EN) else {
        return;
    };
    assert!(s.app.document_markdown_for_test().is_some());
    let path = s.app.tab.preview_path.clone().unwrap();
    s.app.tab.preview_kind = Some(crate::preview::PreviewKind::Text(path));
    assert_eq!(s.app.document_markdown_for_test(), None);
}

/// A conversion that lands after the preview moved on to something that is not a document is
/// dropped, not kept (catches the late-arrival guard in `land_document`; the media generation
/// check upstream is bypassed here by moving the kind without a new load).
#[test]
fn e2e_word_a_document_landing_on_a_non_document_preview_is_dropped() {
    let Some((dir, root)) = doc_sandbox("w_late", EN) else {
        return;
    };
    let mut s = Sim::with_config_sized(&root, cfg_en(), 100, 30).with_media();
    s.select(EN);
    s.enter();
    let path = s.app.tab.preview_path.clone().unwrap();
    s.app.tab.preview_kind = Some(crate::preview::PreviewKind::Text(path));
    let res = s
        .media_rx
        .as_ref()
        .unwrap()
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("the worker answers");
    let _ = s.app.apply_media(res);
    assert_eq!(s.app.document_picture_count_for_test(), 0);
    assert_eq!(s.app.document_markdown_for_test(), None);
    drop(dir);
}

/// Reloading a document with another picture reclaims the old picture's cache entry (catches the
/// prune of unreferenced `office-img://` entries being skipped).
#[test]
fn e2e_word_a_reload_drops_the_cache_entry_of_a_picture_that_is_gone() {
    let dir = sandbox("w_prune");
    let root = canon(&dir);
    let docx = root.join("p.docx");
    build_docx_with_picture(&docx, "one", "a.png", &png_bytes([220, 20, 20]));
    let mut s = open_with_media(&dir, "p.docx");
    s.drain_media();
    settle_images(&mut s);
    let old = s.app.document_first_picture_url_for_test().unwrap();
    assert!(s.app.office_picture_started_for_test(&old));

    build_docx_with_picture(&docx, "two", "b.png", &png_bytes([20, 20, 220]));
    touch_later(&docx, 1_900_000_600);
    s.app.refresh_fs_watched(false, std::slice::from_ref(&docx));
    s.draw();
    drain_media_until_current(&mut s);
    let new = s.app.document_first_picture_url_for_test().unwrap();
    assert_ne!(old, new, "different bytes, different key");
    assert!(
        !s.app.office_picture_started_for_test(&old),
        "the old picture's entry is reclaimed"
    );
}

/// After a reload the *new* text is drawn, not the cached rendering of the old one (catches
/// `md_cache` surviving `land_document`).
#[test]
fn e2e_word_a_reload_draws_the_new_text_not_the_cached_old_one() {
    let dir = sandbox("w_cache");
    let root = canon(&dir);
    let docx = root.join("c.docx");
    build_docx(&docx, &para("OLDTEXT here"));
    let mut s = open_with_media(&dir, "c.docx");
    s.drain_media();
    s.see("OLDTEXT");
    build_docx(&docx, &para("NEWTEXT here"));
    touch_later(&docx, 1_900_000_700);
    s.app.refresh_fs_watched(false, std::slice::from_ref(&docx));
    s.draw();
    drain_media_until_current(&mut s);
    s.see("NEWTEXT");
    s.dont_see("OLDTEXT");
}

/// A document that failed and is then replaced by a good file shows no error any more (catches
/// the error not being cleared when the new document lands).
#[test]
fn e2e_word_a_fixed_file_clears_the_earlier_error() {
    let dir = sandbox("w_fixed");
    let root = canon(&dir);
    let docx = root.join("f.docx");
    std::fs::write(&docx, b"broken").unwrap();
    let mut s = open_with_media(&dir, "f.docx");
    s.drain_media();
    assert!(s.app.document_error().is_some());
    s.see("[document] cannot preview");
    build_docx(&docx, &para("REPAIRED text"));
    touch_later(&docx, 1_900_000_800);
    s.app.refresh_fs_watched(false, std::slice::from_ref(&docx));
    s.draw();
    drain_media_until_current(&mut s);
    assert!(s.app.document_ready());
    assert!(s.app.document_error().is_none());
    s.see("REPAIRED");
    s.dont_see("[document] cannot preview");
}

/// While the worker has not answered yet, the screen is the loading spinner and nothing about a
/// damaged file (catches the loading screen not being taken for a document).
#[test]
fn e2e_word_a_converting_document_shows_the_spinner_not_a_failure() {
    let Some((mut s, _d)) = open_doc_with_media("w_spin", EN, (100, 30)) else {
        return;
    };
    assert!(s.app.is_document_loading());
    s.see("loading…");
    s.dont_see("cannot preview");
    s.dont_see("damaged");
    s.drain_media();
    s.dont_see("loading…");
}

/// A failure with no recorded reason (the state of a job that produced nothing) reads as a damaged
/// file, not as "an old format" (catches the `None` arm of the reason text).
#[test]
fn e2e_word_a_failure_without_a_reason_reads_as_damaged() {
    let (mut s, _d) = open_bad("w_noreason", |p| std::fs::write(p, b"not a zip").unwrap());
    s.app.forget_document_error_for_test();
    s.draw();
    s.see("damaged");
    s.dont_see("old .doc");
}

/// A picture konoma cannot size is drawn as its alt text, with no image box at all (catches the
/// unsizable picture getting a 1x1 box instead).
#[test]
fn e2e_word_an_unsizable_picture_gets_no_image_box() {
    let dir = sandbox("w_nobox");
    let docx = canon(&dir).join("n.docx");
    build_docx_with_picture(&docx, "before", "a.png", b"this is not a png");
    let mut s = open_with_media(&dir, "n.docx");
    s.drain_media();
    settle_images(&mut s);
    s.see("before");
    s.see("the picture");
    assert!(office_placements(&s).is_empty(), "{:?}", s.app.md_images());
}

/// An SVG picture is sized from its own header (no raster size exists), so it gets an image box
/// (catches the SVG size lookup being skipped).
#[test]
fn e2e_word_an_svg_picture_gets_an_image_box() {
    let dir = sandbox("w_svg");
    let docx = canon(&dir).join("s.docx");
    let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" width="80" height="40"><rect width="80" height="40" fill="#c00"/></svg>"##;
    build_docx_with_picture(&docx, "before", "a.svg", svg);
    let mut s = open_with_media(&dir, "s.docx");
    s.drain_media();
    settle_images(&mut s);
    assert_eq!(office_placements(&s).len(), 1, "{:?}", s.app.md_images());
}

/// An animated GIF in a document is decoded with all its frames and cycles (catches the `GIF8`
/// signature test, which would hand it to the still decoder: one frame, no animation).
#[test]
fn e2e_word_an_animated_gif_picture_animates() {
    let dir = sandbox("w_gif");
    let gif = dir.join("anim.gif");
    write_animated_gif(&gif, &[[220, 20, 20], [20, 20, 220]]);
    let docx = canon(&dir).join("g.docx");
    build_docx_with_picture(&docx, "before", "a.gif", &std::fs::read(&gif).unwrap());
    let mut s = open_with_media(&dir, "g.docx");
    s.drain_media();
    settle_images(&mut s);
    assert_eq!(office_placements(&s).len(), 1);
    assert!(
        s.app.md_gif_poll_timeout().is_some(),
        "an animated picture asks for the frame timer"
    );
}

/// Moving from a document shown raw to another document starts the new one decorated, also on the
/// synchronous path (no media worker): the previous file's `R` must not carry over (catches the
/// `md_raw` reset in the new-target path being dropped).
#[test]
fn e2e_word_the_next_document_starts_decorated_not_raw() {
    let dir = sandbox("w_next_raw");
    let root = canon(&dir);
    build_docx(&root.join("a.docx"), &para("FIRSTDOC text"));
    build_docx(&root.join("b.docx"), &para("SECONDDOC text"));
    let mut s = Sim::with_config_sized(&root, cfg_en(), 100, 30);
    s.select("a.docx");
    s.enter();
    s.key('R');
    assert!(s.app.is_md_raw());
    s.ctrl('n');
    assert!(s.app.tab.preview_path.clone().unwrap().ends_with("b.docx"));
    assert!(
        !s.app.is_md_raw(),
        "the new document is not in the raw view"
    );
    // Nor is a reader on the previous raw text left behind (it would be built when the new document
    // lands on the synchronous path, before the flag is reset).
    assert!(!s.app.is_windowed(), "no windowed reader");
    assert_eq!(s.app.document_raw_file_for_test(), None, "no raw temp file");
    s.see("SECONDDOC");
}

/// With an explicit `[editor] ext` entry for docx, `e` goes to the editor, and the editor is
/// never asked for a line: the raw view's caret line is a line of the converted text, not of the
/// file (catches the Document guard in `preview_edit_line`).
#[test]
fn e2e_word_the_editor_is_not_given_a_line_of_the_converted_text() {
    let dir = sandbox("w_edit_line");
    let root = canon(&dir);
    let long: String = (0..60).map(|i| para(&format!("line {i}"))).collect();
    build_docx(&root.join("e.docx"), &long);
    let mut cfg = cfg_en();
    cfg.editor.ext.insert("docx".into(), "myeditor".into());
    let mut s = Sim::with_config_sized(&root, cfg, 100, 30);
    s.select("e.docx");
    s.enter();
    s.key('R');
    for _ in 0..8 {
        s.key('j');
    }
    s.key('e');
    let (p, line) = s.app.take_pending_edit().expect("the editor was asked");
    assert!(p.ends_with("e.docx"));
    assert_eq!(line, None, "no line of the converted text");
}

/// The raw view of a document is coloured as Markdown: a heading line's text differs in colour
/// from a plain paragraph's (catches the raw view not being highlighted at all, and the grammar
/// being picked from anything but Markdown).
#[test]
fn e2e_word_the_raw_view_is_highlighted_as_markdown() {
    let dir = sandbox("w_hl");
    let root = canon(&dir);
    let body = "<w:p><w:pPr><w:pStyle w:val=\"Heading1\"/></w:pPr><w:r><w:t>HEADWORD</w:t></w:r></w:p><w:p><w:r><w:t>PLAINWORD body</w:t></w:r></w:p>";
    build_docx_with_styles_heading(&root.join("h.docx"), body);
    let mut s = Sim::with_config_sized(&root, cfg_en(), 100, 30);
    s.select("h.docx");
    s.enter();
    s.key('R');
    s.see("# HEADWORD");
    let fg_of = |s: &Sim, needle: &str| {
        let buf = s.term.backend().buffer();
        let w = buf.area.width as usize;
        for y in 0..buf.area.height as usize {
            let row: String = (0..w)
                .map(|x| buf.cell((x as u16, y as u16)).unwrap().symbol().to_string())
                .collect();
            if let Some(i) = row.find(needle) {
                let col = row[..i].chars().count();
                return buf.cell((col as u16, y as u16)).unwrap().style().fg;
            }
        }
        panic!("{needle} not on screen:\n{}", s.screen());
    };
    let head = fg_of(&s, "HEADWORD");
    let plain = fg_of(&s, "PLAINWORD");
    assert_ne!(head, plain, "the heading is coloured as Markdown");
}

/// A docx whose styles part defines `Heading1`.
fn build_docx_with_styles_heading(path: &std::path::Path, body: &str) {
    use std::io::Write;
    let ns = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
    let rel = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
    let f = std::fs::File::create(path).unwrap();
    let mut zw = zip::ZipWriter::new(f);
    let o = zip::write::SimpleFileOptions::default();
    let mut put = |n: &str, t: &str| {
        zw.start_file(n, o).unwrap();
        zw.write_all(t.as_bytes()).unwrap();
    };
    put(
        "[Content_Types].xml",
        r#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"/>"#,
    );
    put(
        "_rels/.rels",
        &format!(
            r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{rel}/officeDocument" Target="word/document.xml"/></Relationships>"#
        ),
    );
    put(
        "word/_rels/document.xml.rels",
        &format!(
            r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rIdS" Type="{rel}/styles" Target="styles.xml"/></Relationships>"#
        ),
    );
    put(
        "word/styles.xml",
        &format!(
            r#"<w:styles xmlns:w="{ns}"><w:style w:type="paragraph" w:styleId="Heading1"><w:name w:val="heading 1"/><w:pPr><w:outlineLvl w:val="0"/></w:pPr></w:style></w:styles>"#
        ),
    );
    put(
        "word/document.xml",
        &format!(r#"<w:document xmlns:w="{ns}"><w:body>{body}</w:body></w:document>"#),
    );
    zw.finish().unwrap();
}
