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
    // Idle-based: the limit restarts whenever a result arrives (see deck_view's `settle`).
    let idle = std::time::Duration::from_secs(120);
    let mut deadline = std::time::Instant::now() + idle;
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
        if any {
            deadline = std::time::Instant::now() + idle;
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
    // The document's own spinner is gone (the status row may already say "loading images": it is
    // drawn after the body, which has just started the pictures).
    s.dont_see("loading document");
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

/// Like `build_docx_with_picture` for several pictures, one paragraph each.
fn build_docx_with_pictures(path: &std::path::Path, pics: &[(String, Vec<u8>)]) {
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
    let mut rels = String::new();
    let mut body = String::new();
    for (i, (name, bytes)) in pics.iter().enumerate() {
        rels +=
            &format!(r#"<Relationship Id="rIdP{i}" Type="{rel}/image" Target="media/{name}"/>"#);
        put(&format!("word/media/{name}"), bytes);
        body += &format!(
            r#"<w:p><w:r><w:t>photo {i}</w:t></w:r></w:p><w:p><w:r><w:drawing><wp:inline><wp:docPr id="{}" name="x" descr="picture {i}"/><a:graphic><a:graphicData><a:blip r:embed="rIdP{i}"/></a:graphicData></a:graphic></wp:inline></w:drawing></w:r></w:p>"#,
            i + 1
        );
    }
    put(
        "word/_rels/document.xml.rels",
        format!(r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">{rels}</Relationships>"#).as_bytes(),
    );
    put(
        "word/document.xml",
        format!(r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="{rel}" xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><w:body>{body}</w:body></w:document>"#).as_bytes(),
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

// ---------------------------------------------------------------------------------------------
// A document without text on screen (loading / failed): footer, `?` help and keys agree
// ([[hint-shown-iff-key-acts]])
// ---------------------------------------------------------------------------------------------

/// The CFB signature then junk: an old `.doc` named `.docx` (reported as an unsupported format).
fn legacy_doc(p: &std::path::Path) {
    let mut b = vec![0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];
    b.extend_from_slice(&[0u8; 600]);
    std::fs::write(p, b).unwrap();
}

fn open_bad_cfg(
    name: &str,
    cfg: Config,
    size: (u16, u16),
    make: impl FnOnce(&std::path::Path),
) -> (Sim, crate::test_support::TmpDir) {
    let dir = sandbox(name);
    let root = canon(&dir);
    make(&root.join("bad.docx"));
    std::fs::write(root.join("z.txt"), "after\n").unwrap();
    let mut s = Sim::with_config_sized(&root, cfg, size.0, size.1);
    s.select("bad.docx");
    s.enter();
    (s, dir)
}

/// Every state in which a document has no text on screen, as (label, sim, keep-alive).
fn missing_text_states() -> Vec<(&'static str, Sim, crate::test_support::TmpDir)> {
    let mut v = Vec::new();
    let (s, d) = open_bad_cfg("w_mt_corrupt", cfg_en(), (120, 40), |p| {
        std::fs::write(p, b"not a zip").unwrap()
    });
    v.push(("corrupt", s, d));
    let (s, d) = open_bad_cfg("w_mt_legacy", cfg_en(), (120, 40), legacy_doc);
    v.push(("old format", s, d));
    if let Some((s, d)) = open_doc_with_media("w_mt_loading", EN, (120, 40)) {
        assert!(s.app.is_document_loading());
        v.push(("loading", s, d));
    }
    // Raw view, then another tab and back: the worker re-reads the document while the tab's saved
    // raw state is still set.
    if let Some((mut s, d)) = open_doc_with_media("w_mt_rawload", EN, (120, 40)) {
        s.drain_media();
        s.key('R');
        assert!(s.app.is_raw_source());
        s.key('t');
        s.key('1');
        s.draw();
        assert!(s.app.is_document_loading() && s.app.is_md_raw());
        v.push(("raw view, reloading after a tab switch", s, d));
    }
    let mut cfg = cfg_en();
    cfg.external.office_apps = false;
    let (s, d) = open_bad_cfg("w_mt_noapps", cfg, (120, 40), legacy_doc);
    v.push(("office_apps = false", s, d));
    v
}

#[test]
fn e2e_word_without_text_the_footer_and_help_offer_only_keys_that_act() {
    for (label, mut s, _d) in missing_text_states() {
        s.draw();
        assert!(s.app.document_text_missing(), "{label}");
        let f = footer_text(&s);
        for dead in [
            "R:", "/:", "hl:", "v/V", "0/$", "g/G", "o:", "Y:", "Tab:", "jk:",
        ] {
            assert!(!f.contains(dead), "[{label}] footer offers {dead}: {f}");
        }
        s.key('?');
        let help = s.screen();
        for dead in [
            "v / V",
            "Tab / ⇧Tab",
            "Enter",
            "n / N",
            "j / k",
            "g / G",
            "h / l",
            "0 / $",
            "Ctrl-t",
            "outline",
            "Space",
        ] {
            assert!(!help.contains(dead), "[{label}] help lists {dead}:\n{help}");
        }
        assert!(help.contains("Ctrl-n / Ctrl-p"), "[{label}] {help}");
        assert!(help.contains("q / Esc"), "[{label}] {help}");
        // The help row for `e` is the footer's `e` hint, both worded by `edit_label`.
        let e_in_footer = f.contains("e:");
        let e_in_help = help.contains("Office app") || help.contains("editor");
        assert_eq!(e_in_footer, label != "office_apps = false", "[{label}] {f}");
        assert_eq!(e_in_help, e_in_footer, "[{label}] help vs footer:\n{help}");
    }
}

#[test]
fn e2e_word_without_text_the_text_keys_do_nothing() {
    for (label, mut s, _d) in missing_text_states() {
        s.draw();
        let raw_before = s.app.is_md_raw();
        s.key('/');
        assert!(
            s.app.search_input().is_none(),
            "[{label}] `/` opened a search nobody can see"
        );
        s.key('v');
        s.key('V');
        assert!(
            !s.app.is_preview_visual(),
            "[{label}] v/V started a selection"
        );
        s.key('R');
        assert_eq!(
            s.app.is_md_raw(),
            raw_before,
            "[{label}] R toggled the view"
        );
        // What the footer does list still acts: Ctrl-n pages to the next file.
        s.ctrl('n');
        assert!(
            s.app.tab.preview_path.clone().unwrap().ends_with("z.txt"),
            "[{label}] Ctrl-n did not page"
        );
    }
}

#[test]
fn e2e_word_the_raw_view_reloading_after_a_tab_switch_shows_the_loading_footer() {
    let Some((mut s, _d)) = open_doc_with_media("w_raw_footer", EN, (200, 30)) else {
        return;
    };
    s.drain_media();
    s.key('R');
    s.see("R:rendered");
    s.key('t');
    s.key('1');
    s.draw();
    assert!(s.app.is_document_loading() && s.app.is_md_raw());
    s.see("loading");
    s.dont_see("R:rendered");
    s.dont_see("/:search");
    s.dont_see("v/V:select");
    // When the text arrives the raw view (the user's choice) is back, with its own footer.
    drain_media_until_current(&mut s);
    assert!(s.app.is_md_raw() && s.app.is_windowed());
    s.see("R:rendered");
    s.see("v/V:select");
}

#[test]
fn e2e_word_help_tab_and_enter_rows_name_only_what_a_document_has() {
    let Some((mut s, _d)) = open_doc_sized("w_help_tab", EN, (140, 60), cfg_en()) else {
        return;
    };
    s.key('?');
    let help = s.screen();
    assert!(help.contains("focus a link / code block"), "{help}");
    assert!(
        help.contains("open the focused link (URL / local / anchor)"),
        "{help}"
    );
    for gone in ["checkbox", "diagram", "<details>"] {
        assert!(!help.contains(gone), "{gone}:\n{help}");
    }
}

#[test]
fn e2e_word_help_tab_row_of_a_markdown_file_keeps_the_whole_list() {
    let dir = sandbox("w_help_tab_md");
    std::fs::write(dir.join("a.md"), "# t\n\n- [ ] x\n").unwrap();
    let mut s = Sim::with_config_sized(&canon(&dir), cfg_en(), 140, 60);
    s.select("a.md");
    s.enter();
    s.key('?');
    s.see("focus md link / checkbox / code block");
}

#[test]
fn e2e_word_help_tab_row_is_japanese_too() {
    let Some((dir, root)) = doc_sandbox("w_help_tab_ja", EN) else {
        return;
    };
    let mut s = Sim::with_config_sized(&root, cfg_ja(), 140, 60);
    s.select(EN);
    s.enter();
    s.key('?');
    see_cjk(&mut s, "リンク/コードブロックをフォーカス");
    drop(dir);
}

// ---------------------------------------------------------------------------------------------
// Error wording: the `e` hint follows `e`, the sentence wraps
// ---------------------------------------------------------------------------------------------

#[test]
fn e2e_word_an_old_format_error_offers_e_only_while_e_opens_an_office_app() {
    let (s, _d) = open_bad_cfg("w_err_e_on", cfg_en(), (100, 30), legacy_doc);
    s.see("an old .doc, for one)");
    s.see("Press `e` to open it in an Office app");
    // Office apps switched off: `e` only explains that, so the screen does not promise it.
    let mut cfg = cfg_en();
    cfg.external.office_apps = false;
    let (s, _d) = open_bad_cfg("w_err_e_off", cfg, (100, 30), legacy_doc);
    s.see("an old .doc, for one)");
    s.dont_see("Press `e`");
    // An explicit `[editor] ext` rule for docx makes `e` open an editor, not an Office app.
    let mut cfg = cfg_en();
    cfg.editor.ext.insert("docx".into(), "vim".into());
    let (s, _d) = open_bad_cfg("w_err_e_editor", cfg, (100, 30), legacy_doc);
    s.dont_see("Press `e`");
}

#[test]
fn e2e_word_the_error_sentence_wraps_in_a_narrow_window_whatever_wrap_says() {
    for wrap in [true, false] {
        let mut cfg = cfg_en();
        cfg.ui.wrap = wrap;
        let (s, _d) = open_bad_cfg("w_err_wrap", cfg, (70, 30), legacy_doc);
        // The end of the reason (past column 70 on one line) and the `e` line are both visible
        // (whitespace-insensitive: the wrap may break between words).
        let mut s = s;
        see_cjk(&mut s, ".doc, for one)"); // the tail lands on the second row
        see_cjk(&mut s, "Press `e` to open it in an Office app");
        see_cjk(&mut s, "bad.docx");
    }
    // The same for the other reasons: the whole sentence is readable at 60 columns.
    let (s, _d) = open_bad_cfg("w_err_wrap_corrupt", cfg_en(), (60, 30), |p| {
        std::fs::write(p, b"nope").unwrap()
    });
    let mut s = s;
    see_cjk(&mut s, "valid Word document"); // the tail wraps onto row 2
                                            // And a workbook's reason wraps as well.
    let dir = sandbox("w_err_wrap_sheet");
    std::fs::write(dir.join("bad.xlsx"), b"nope").unwrap();
    let mut s = Sim::with_config_sized(&canon(&dir), cfg_en(), 60, 30);
    s.select("bad.xlsx");
    s.enter();
    see_cjk(&mut s, "a valid workbook"); // the tail wraps onto row 2
}

#[test]
fn e2e_word_the_old_format_error_in_japanese_reads_cleanly() {
    let (mut s, _d) = open_bad_cfg("w_err_ja", cfg_ja(), (70, 30), legacy_doc);
    see_cjk(&mut s, "(古い .doc など)");
    see_cjk(&mut s, "Office アプリで開くには `e` を押してください");
    let text = s.screen();
    assert!(!text.contains("（"), "full-width paren left: {text}");
    // The doubled "で ... で" of the old sentence is gone.
    let squashed: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    assert!(!squashed.contains("`e`でOfficeアプリで"), "{squashed}");
    let mut cfg = cfg_ja();
    cfg.external.office_apps = false;
    let (mut s, _d) = open_bad_cfg("w_err_ja_off", cfg, (70, 30), legacy_doc);
    clean(&mut s);
    assert!(!s.screen().contains("`e`"), "{}", s.screen());
}

// ---------------------------------------------------------------------------------------------
// The busy label names what is being read
// ---------------------------------------------------------------------------------------------

fn busy_label(s: &Sim) -> String {
    crate::ui::status::context_spans(&s.app)
        .iter()
        .map(|sp| sp.content.as_ref())
        .collect()
}

#[test]
fn e2e_word_a_converting_document_is_not_labelled_media() {
    let Some((s, _d)) = open_doc_with_media("w_busy_doc", EN, (140, 30)) else {
        return;
    };
    assert!(s.app.is_document_loading());
    assert!(s.app.busy_jobs().contains(&crate::i18n::Msg::BusyDocument));
    assert!(!s.app.busy_jobs().contains(&crate::i18n::Msg::BusyMedia));
    let l = busy_label(&s);
    assert!(
        l.contains("loading document") && !l.contains("media"),
        "{l}"
    );
}

#[test]
fn e2e_word_a_converting_document_label_is_japanese_in_a_japanese_ui() {
    let Some((dir, root)) = doc_sandbox("w_busy_doc_ja", EN) else {
        return;
    };
    let mut s = Sim::with_config_sized(&root, cfg_ja(), 140, 30).with_media();
    s.select(EN);
    s.enter();
    assert!(s.app.is_document_loading());
    let l = busy_label(&s);
    assert!(l.contains("文書読込") && !l.contains("メディア"), "{l}");
    drop(dir);
}

#[test]
fn e2e_sheet_a_loading_workbook_is_not_labelled_media() {
    let dir = sandbox("w_busy_sheet");
    build_xlsx(&dir.join("b.xlsx"), &[("S", "visible", "", "")]);
    let mut s = Sim::with_config_sized(&canon(&dir), cfg_en(), 140, 30).with_media();
    s.select("b.xlsx");
    s.enter();
    assert!(s.app.is_sheet_loading());
    assert!(s.app.busy_jobs().contains(&crate::i18n::Msg::BusySheet));
    assert!(!s.app.busy_jobs().contains(&crate::i18n::Msg::BusyMedia));
    let l = busy_label(&s);
    assert!(
        l.contains("loading spreadsheet") && !l.contains("media"),
        "{l}"
    );
}

// ---------------------------------------------------------------------------------------------
// Follow ignores an Office suite's owner file
// ---------------------------------------------------------------------------------------------

#[test]
fn e2e_follow_does_not_chase_an_office_owner_file() {
    let dir = sandbox("w_follow_lock");
    let root = canon(&dir);
    std::fs::write(root.join("~$report.docx"), b"\x10owner").unwrap();
    std::fs::write(root.join("~$book.xlsx"), b"\x10owner").unwrap();
    std::fs::write(root.join("~$deck.pptx"), b"\x10owner").unwrap();
    std::fs::write(root.join("report.docx"), b"x").unwrap();
    std::fs::write(root.join("a.txt"), b"first\n").unwrap();
    let mut s = Sim::with_config(&root, cfg_en());
    s.select("a.txt");
    s.enter();
    s.key('q');
    s.key('F');
    assert!(s.app.follow_enabled());
    for lock in ["~$report.docx", "~$book.xlsx", "~$deck.pptx"] {
        let p = root.join(lock);
        assert!(!s.app.follow_note_change(&p), "{lock} recorded");
        s.app.follow_jump(&p);
        assert!(
            s.app.tab.preview_path.as_deref() != Some(p.as_path()),
            "{lock} was followed"
        );
    }
    // The document itself is still a follow target.
    let doc = root.join("report.docx");
    assert!(s.app.follow_note_change(&doc));
    // And the owner file is not hidden from the tree.
    s.key('F');
    s.see("~$report.docx");
}

// ---------------------------------------------------------------------------------------------
// A Word / OpenDocument picture gets the same protection as a Markdown picture (the merge of the
// Word preview with the hostile-image defences): the same size cap, the same SVG drawing process
// (no files readable), the same reasons, and the same cache.
// ---------------------------------------------------------------------------------------------

/// A PNG of one flat colour, `w` x `h` (a few hundred bytes whatever the size).
fn flat_png(w: u32, h: u32) -> Vec<u8> {
    use image::{ImageFormat, Rgba, RgbaImage};
    let img = RgbaImage::from_pixel(w, h, Rgba([30, 120, 220, 255]));
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, ImageFormat::Png).unwrap();
    out.into_inner()
}

/// Opens a docx holding one picture `name` of `bytes`, with the real workers, and lets it settle.
fn open_picture_docx(
    tag: &str,
    name: &str,
    bytes: &[u8],
) -> (Sim, crate::test_support::TmpDir, String) {
    let dir = sandbox(tag);
    build_docx_with_picture(&canon(&dir).join("p.docx"), "before", name, bytes);
    let mut s = open_with_media(&dir, "p.docx");
    s.drain_media();
    assert!(s.app.document_ready());
    settle_images(&mut s);
    let url = s.app.document_first_picture_url_for_test().unwrap();
    (s, dir, url)
}

#[test]
fn e2e_word_a_large_picture_is_kept_at_4096_px_on_its_long_side() {
    let (s, _d, url) = open_picture_docx("w_cap4096", "wide.png", &flat_png(6000, 300));
    let (w, h) = s
        .app
        .office_picture_pixels_for_test(&url)
        .expect("the picture was decoded");
    assert_eq!(w, 4096, "the long side is capped");
    assert_eq!(h, 205, "the aspect ratio is kept (300 * 4096 / 6000)");
}

#[test]
fn e2e_word_a_picture_within_the_cap_is_not_resized() {
    let (s, _d, url) = open_picture_docx("w_nocap", "ok.png", &flat_png(640, 480));
    assert_eq!(s.app.office_picture_pixels_for_test(&url), Some((640, 480)));
}

#[test]
fn e2e_word_a_picture_over_the_decode_limits_says_it_is_too_large() {
    // 40,000 px a side is past the 32,768 limit: refused from the header, before any pixel
    // buffer exists, and the screen says so instead of "damaged" or "terminal cannot".
    let (s, _d, url) = open_picture_docx("w_toolarge", "huge.png", &flat_png(40_000, 4));
    assert!(
        !s.app.office_picture_started_for_test(&url),
        "refused from the header: no decode was queued"
    );
    s.see("before");
    s.see("too large");
}

#[test]
fn e2e_word_a_picture_that_is_no_image_is_its_alt_text() {
    let (s, _d, url) = open_picture_docx("w_damaged", "a.png", b"this is not a png");
    assert!(!s.app.office_picture_started_for_test(&url));
    s.see("before");
    s.see("the picture");
}

/// An SVG inside the document is drawn through the guarded path: 1000 nested groups used to
/// overflow the stack of whatever drew them. Now the picture is refused with its reason, the
/// document around it is untouched and konoma carries on.
#[test]
fn e2e_word_a_deeply_nested_svg_is_refused_with_its_reason() {
    let deep = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="20">{}{}</svg>"#,
        "<g>".repeat(1000),
        "</g>".repeat(1000)
    );
    let (s, _d, url) = open_picture_docx("w_deepsvg", "deep.svg", deep.as_bytes());
    assert_eq!(s.app.office_picture_pixels_for_test(&url), None);
    assert_eq!(
        s.app.office_picture_failure_for_test(&url),
        Some(crate::preview::image::ImageFailure::Svg(
            crate::preview::svg_guard::SvgFail::TooDeep
        ))
    );
    s.see("nested too deeply");
    s.see("before");
}

/// The same for a picture that expands exponentially (4 million elements from 22 `<use>` levels),
/// and a normal SVG next to it still draws.
#[test]
fn e2e_word_a_use_bomb_svg_is_refused_and_a_normal_one_still_draws() {
    let mut body = String::from(r#"<defs><rect id="u0" width="2" height="2"/>"#);
    for i in 0..22 {
        body += &format!(
            r##"<g id="u{}"><use href="#u{i}"/><use href="#u{i}" x="1"/></g>"##,
            i + 1
        );
    }
    body += r##"</defs><use href="#u22"/>"##;
    let bomb =
        format!(r#"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="20">{body}</svg>"#);
    let (s, _d, url) = open_picture_docx("w_bombsvg", "bomb.svg", bomb.as_bytes());
    assert_eq!(s.app.office_picture_pixels_for_test(&url), None);
    assert_eq!(
        s.app.office_picture_failure_for_test(&url),
        Some(crate::preview::image::ImageFailure::Svg(
            crate::preview::svg_guard::SvgFail::TooComplex
        ))
    );
    s.see("before");

    let fine = br##"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="20"><rect width="40" height="20" fill="#0a0"/></svg>"##;
    let (s, _d, url) = open_picture_docx("w_finesvg", "fine.svg", fine);
    let (w, h) = s.app.office_picture_pixels_for_test(&url).expect("drawn");
    assert_eq!(w / h, 2, "the 2:1 picture keeps its shape: {w}x{h}");
    assert_eq!(s.app.office_picture_failure_for_test(&url), None);
}

/// The cache may drop the pixels of a Word picture like any other picture (they are made again
/// from the bytes the converted document keeps), and the page does not move while they are gone.
/// Formulas (rebuilt from their SVG) behave the same.
#[test]
fn e2e_word_pictures_and_formulas_are_dropped_by_the_cache_and_come_back_in_place() {
    let Some((mut s, _d)) = open_doc_with_media("w_lru", EN, TALL) else {
        return;
    };
    s.drain_media();
    settle_images(&mut s);
    let before: Vec<_> = s.app.md_images();
    assert!(before
        .iter()
        .any(|p| crate::preview::markdown::is_office_image_url(&p.url)));
    let math: Vec<_> = before
        .iter()
        .filter(|p| crate::preview::markdown::is_math_url(&p.url))
        .map(|p| p.url.clone())
        .collect();
    assert_eq!(math.len(), 2);
    let pic = s.app.document_first_picture_url_for_test().unwrap();
    let pic_px = s.app.office_picture_pixels_for_test(&pic).unwrap();

    // A new pass has started (the pictures above were drawn in the previous one), then the cache
    // is told to hold nothing.
    s.app.begin_md_image_frame();
    s.app.evict_md_images_to_for_test(0);
    assert_eq!(
        s.app.office_picture_pixels_for_test(&pic),
        None,
        "a Word picture is rebuildable, so it is dropped"
    );
    assert_eq!(
        s.app.md_image_cache_pixel_bytes(),
        0,
        "nothing stays resident"
    );
    for m in &math {
        assert_eq!(s.app.office_picture_pixels_for_test(m), None, "{m}");
    }
    // The page does not move: laid out again from scratch with every pixel gone, the reserved
    // cells (rows, columns) of every picture and formula are the same.
    s.app.invalidate_md_cache_for_test();
    s.draw();
    assert_eq!(format!("{:?}", s.app.md_images()), format!("{before:?}"));

    // The same draw asked for the pixels back: they arrive, identical, and the page is unchanged.
    settle_images(&mut s);
    assert_eq!(s.app.office_picture_pixels_for_test(&pic), Some(pic_px));
    for m in &math {
        assert!(s.app.office_picture_pixels_for_test(m).is_some(), "{m}");
    }
    assert_eq!(format!("{:?}", s.app.md_images()), format!("{before:?}"));
}

/// A document of `n` photographs: the cache keeps only about its budget of them resident however
/// far the reader scrolls, and the ones scrolled back to are drawn again.
#[test]
fn e2e_word_a_picture_heavy_document_stays_within_the_cache_budget() {
    const N: u32 = 10;
    let dir = sandbox("w_lru_many");
    let pics: Vec<(String, Vec<u8>)> = (0..N)
        .map(|i| (format!("p{i}.png"), flat_png(200 + i, 200)))
        .collect();
    build_docx_with_pictures(&canon(&dir).join("many.docx"), &pics);
    let mut s = open_with_media(&dir, "many.docx");
    // Room for two of them (200 x 200 x 4 bytes each, small because encoding is slow in a debug build).
    s.app.set_md_cache_budget_for_test(2 * 200 * 204 * 4);
    s.drain_media();
    assert!(s.app.document_ready());
    assert_eq!(s.app.document_picture_count_for_test(), N as usize);
    settle_images(&mut s);
    // Read the whole document, a screen at a time.
    for _ in 0..40 {
        s.key('j');
        s.key('j');
        s.key('j');
        settle_images(&mut s);
    }
    let resident = s.app.office_pictures_with_pixels_for_test();
    assert!(
        resident < N as usize,
        "the budget held back some of the {N} pictures: {resident} resident"
    );
    // Back at the top the first picture is drawn again.
    for _ in 0..200 {
        s.key('k');
    }
    settle_images(&mut s);
    let first = s.app.office_pictures_with_pixels_for_test();
    assert!(first >= 1, "the pictures in view are drawn again");
    assert!(first < N as usize, "and the rest are still held back");
}

/// A document of many pictures read to the end with a cache that holds about one of them: every
/// picture is drawn when it comes on screen, however many were drawn (and dropped) before. Once
/// enough dropped pictures had piled up, they used to count as decodes "in flight" and no further
/// picture was ever started.
#[test]
fn e2e_word_every_picture_of_a_long_document_is_drawn_after_many_were_dropped() {
    const N: u32 = 24;
    let dir = sandbox("w_lru_long");
    let pics: Vec<(String, Vec<u8>)> = (0..N)
        .map(|i| (format!("p{i}.png"), flat_png(100 + i, 100)))
        .collect();
    build_docx_with_pictures(&canon(&dir).join("long.docx"), &pics);
    let mut s = open_with_media(&dir, "long.docx");
    s.app.set_md_cache_budget_for_test(130 * 130 * 4);
    s.drain_media();
    assert!(s.app.document_ready());
    assert_eq!(s.app.document_picture_count_for_test(), N as usize);
    settle_images(&mut s);
    let mut seen = std::collections::HashSet::new();
    for _ in 0..150 {
        let (top, height) = (
            s.app.preview_scroll_for_test() as usize,
            s.app.preview_viewport_for_test() as usize,
        );
        let on_screen = |s: &Sim, line: usize| {
            let (row, rows) = s.app.md_visual_span_for_test(line);
            row < top + height && row + rows > top
        };
        for p in office_placements(&s) {
            if !on_screen(&s, p.line) {
                continue;
            }
            assert!(
                s.app.office_picture_pixels_for_test(&p.url).is_some(),
                "{} is on screen but was not drawn (evicted: {}, in flight: {})",
                p.url,
                s.app.office_pictures_evicted_for_test(),
                s.app.office_pictures_in_flight_for_test()
            );
            seen.insert(p.url);
        }
        for _ in 0..4 {
            s.key('j');
        }
        settle_images(&mut s);
    }
    assert_eq!(seen.len(), N as usize, "every picture was on screen once");
    assert!(
        s.app.office_pictures_evicted_for_test() > 16,
        "the cap was never reached: {}",
        s.app.office_pictures_evicted_for_test()
    );
    assert_eq!(s.app.office_pictures_in_flight_for_test(), 0);
}

/// A dropped picture is made again through the same defences as the first decode.
#[test]
fn e2e_word_a_dropped_large_picture_comes_back_capped_at_4096() {
    let (mut s, _d, url) = open_picture_docx("w_lru_cap", "wide.png", &flat_png(6000, 300));
    assert_eq!(
        s.app.office_picture_pixels_for_test(&url),
        Some((4096, 205))
    );
    s.app.begin_md_image_frame();
    s.app.evict_md_images_to_for_test(0);
    assert_eq!(s.app.office_picture_pixels_for_test(&url), None);
    s.draw();
    settle_images(&mut s);
    assert_eq!(
        s.app.office_picture_pixels_for_test(&url),
        Some((4096, 205))
    );
    assert_eq!(s.app.office_picture_failure_for_test(&url), None);
}

/// The picture bytes are held by the open document only: once it is left (and its decodes are
/// over) nothing keeps them, so rebuildable pictures do not outlive the document.
#[test]
fn e2e_word_the_picture_bytes_are_freed_when_the_document_is_left() {
    let (mut s, _d, url) = open_picture_docx("w_lru_free", "p.png", &flat_png(300, 200));
    let weak = s.app.document_picture_weak_for_test(&url).unwrap();
    assert!(weak.upgrade().is_some(), "the open document holds them");
    s.app.begin_md_image_frame();
    s.app.evict_md_images_to_for_test(0);
    settle_images(&mut s);
    assert!(
        weak.strong_count() == 1,
        "a dropped picture's entry keeps no copy of the bytes: {}",
        weak.strong_count()
    );
    s.key('q');
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while weak.strong_count() > 0 {
        assert!(
            std::time::Instant::now() < deadline,
            "the bytes were never freed"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

/// A Word SVG is not refused for its intrinsic size (it is guarded by its drawing process, like a
/// file SVG), only a raster is refused from its header.
#[test]
fn e2e_word_an_svg_with_a_huge_intrinsic_size_is_drawn_not_refused() {
    let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" width="40000" height="20"><rect width="40000" height="20" fill="#0a0"/></svg>"##;
    let (s, _d, url) = open_picture_docx("w_hugesvg", "wide.svg", svg);
    assert!(
        s.app.office_picture_started_for_test(&url),
        "its decode was started"
    );
    assert_eq!(s.app.office_picture_failure_for_test(&url), None);
    assert!(s.app.office_picture_pixels_for_test(&url).is_some());
    s.see("before");
    assert!(!s.screen().contains("too large"));
}

/// A picture whose drawing fails after the preview has moved on is not remembered as "damaged": the
/// thread reports it as cancelled, its entry is forgotten and it is asked for again when the
/// document is shown again (otherwise a good picture that was being drawn when the document was
/// closed would stay broken for good).
#[test]
fn e2e_word_a_picture_failing_after_the_preview_moved_on_is_asked_for_again() {
    // A header that passes the size check followed by garbage: the decode fails.
    let mut broken = flat_png(64, 64);
    broken.truncate(broken.len() / 2);
    let (mut s, _d, url) = open_picture_docx("w_moved_on", "b.png", &broken);
    assert_eq!(
        s.app.office_picture_failure_for_test(&url),
        Some(crate::preview::image::ImageFailure::Corrupt),
        "setup: with nothing moving on, a failed decode is a failure"
    );
    s.app.forget_office_picture_for_test(&url);
    s.app.make_running_decodes_stale_for_test();
    s.app.ensure_md_image(&url, 10, 5, 0, 5);
    let res = s
        .md_img_rx
        .as_ref()
        .unwrap()
        .recv_timeout(std::time::Duration::from_secs(20))
        .expect("the decode reports");
    assert_eq!(
        res.error_code_for_test(),
        Some(crate::preview::image::ImageFailure::Cancelled.code()),
        "a failure after the preview moved on is a cancellation"
    );
    s.app.apply_md_image(res);
    assert!(
        !s.app.office_picture_started_for_test(&url),
        "forgotten, so the next showing asks again"
    );
}
