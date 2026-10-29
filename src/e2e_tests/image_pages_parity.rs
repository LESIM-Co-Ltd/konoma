//! A picture made of pages (a PDF) in the konoma host and in the standalone app: `J`/`K` turn the
//! page in both, stop at both ends, show the page number in the same place and words, and the
//! footer and the help name the page keys exactly when the host does. Compared against the real
//! `PdfPlugin` on a generated PDF, on a `TestBackend` with a halfblocks picker; the host's
//! answers are applied by the test itself and the app is driven only when it is idle, so nothing
//! depends on timing.

use super::image_parity::{
    app_zoom, host_zoom, image_section, shot, Shot, WhenIdle, HOST_ONLY, HOST_ONLY_ROWS,
};
use super::table_parity::{help_lines, hints, panel_lines, presses, same_row, APP_H, HOST_H, W};
use super::*;
use konoma_plugin::app::{
    key_from_crossterm, open_file, run_with_pictures, AppCx, AppEvent, AppKeys, Pictures,
};
use konoma_plugin::TermCaps;
use ratatui_image::picker::Picker;
use std::sync::Arc;

/// A PDF of `n` pages, each filled with another color (so a turn changes the picture).
fn make_pdf(dir: &crate::test_support::TmpDir, name: &str, n: usize) {
    const FILLS: [&[u8]; 4] = [
        b"1 0 0 rg 0 0 200 300 re f",
        b"0 0 1 rg 0 0 200 300 re f",
        b"0 1 0 rg 0 0 200 300 re f",
        b"1 1 0 rg 0 0 200 300 re f",
    ];
    let pages: Vec<(u32, u32, &[u8])> = (0..n).map(|i| (200, 300, FILLS[i % 4])).collect();
    std::fs::write(dir.join(name), konoma_test_support::build_pdf(&pages)).unwrap();
}

fn app_pdf_shot(path: &std::path::Path, h: u16, spec: &str, pictures: bool) -> Shot {
    let mut opened = open_file(
        vec![Arc::new(konoma_media::PdfPlugin)],
        path,
        "en",
        TermCaps::new(),
    )
    .expect("opens");
    let mut term = Terminal::new(TestBackend::new(W, h)).unwrap();
    let cx = AppCx {
        lang: "en".into(),
        term: TermCaps::new(),
        title: "t".into(),
        unavailable: opened.unavailable.clone(),
        ..AppCx::default()
    };
    let keys = presses(spec)
        .into_iter()
        .map(|(code, mods)| {
            AppEvent::Key(key_from_crossterm(&KeyEvent::new(code, mods)).expect("a key"))
        })
        .collect();
    let mut events = WhenIdle { keys, turns: 0 };
    let pics = pictures.then(|| Pictures::new(Picker::halfblocks(), "pdf", &opened.path, 800));
    run_with_pictures(
        &mut term,
        &mut *opened.session,
        &mut events,
        &cx,
        &AppKeys::defaults(),
        pics,
    )
    .unwrap();
    assert!(events.keys.is_empty(), "the app ended early: {spec}");
    shot(&term)
}

fn host_pdf_shot(root: &std::path::Path, file: &str, h: u16, spec: &str, picker: bool) -> Shot {
    let mut s = Sim::with_config_sized(root, cfg_en(), W, h);
    let (req_tx, mut req_rx) = tokio::sync::mpsc::unbounded_channel();
    if picker {
        s = s.with_picker().with_media();
        s.app.attach_image_backend(Picker::halfblocks(), req_tx);
    }
    s.select(file);
    s.enter();
    if picker {
        s.drain_media();
    }
    let settle = |s: &mut Sim, req_rx: &mut tokio::sync::mpsc::UnboundedReceiver<_>| {
        s.draw();
        while let Ok(req) = req_rx.try_recv() {
            let req: ratatui_image::thread::ResizeRequest = req;
            s.app.apply_image_resize(req.resize_encode());
            s.draw();
        }
    };
    settle(&mut s, &mut req_rx);
    for (code, mods) in presses(spec) {
        let before = s.app.page_indicator();
        s.press(code, mods);
        assert!(!s.quit, "the host ended early: {spec}");
        // A turn that moved asks for the new page's picture: wait for it.
        if picker && s.app.page_indicator() != before {
            s.drain_media();
        }
        settle(&mut s, &mut req_rx);
    }
    shot(&s.term)
}

/// `2/5` as a footer or status word.
fn page_word(w: &str) -> bool {
    w.split_once('/')
        .is_some_and(|(a, b)| a.parse::<u32>().is_ok() && b.parse::<u32>().is_ok())
}

fn host_page(rows: &[String]) -> Option<String> {
    rows[0]
        .split_whitespace()
        .find(|w| page_word(w))
        .map(str::to_string)
}

fn app_page(rows: &[String]) -> Option<String> {
    hints(rows.last()?).into_iter().find(|w| page_word(w))
}

/// The footer's key hints (no zoom, page number or title).
fn key_hints(footer: &str) -> Vec<String> {
    let mut v = hints(footer);
    v.retain(|w| !page_word(w) && !(w.starts_with('x') && w[1..].parse::<f64>().is_ok()));
    v
}

/// What only the host says on a paged picture, on top of `HOST_ONLY`.
const HOST_ONLY_PAGED: &[&str] = &["O:pages"];

const SPECS: &[&str] = &[
    "",
    "J",
    "J J",
    "J J J",
    "J J J J",
    "K",
    "J K",
    "J J K",
    "J J K K",
    "+ J",
    "+ + J",
    "J +",
    "J h",
    "J J J J K K K K K",
];

#[test]
fn the_page_turn_the_number_and_the_footer_agree_with_the_host() {
    let dir = sandbox("image_pages_parity");
    make_pdf(&dir, "three.pdf", 3);
    let root = canon(&dir);
    let (mut checked, mut moved, mut hinted) = (0, 0, 0);
    let mut pages = std::collections::BTreeSet::new();
    let mut seen_host_only = std::collections::BTreeSet::new();
    let at_fit = host_pdf_shot(&root, "three.pdf", HOST_H, "", true);
    for spec in SPECS {
        let what = format!("three.pdf [{spec}]");
        let h = host_pdf_shot(&root, "three.pdf", HOST_H, spec, true);
        let a = app_pdf_shot(&root.join("three.pdf"), APP_H, spec, true);
        assert!(h.cells.len() > 400, "{what}: the host frame is there");
        assert_eq!(
            h.cells.len(),
            a.cells.len(),
            "{what}: the frames differ in size"
        );
        if h.cells != a.cells {
            let first = h
                .cells
                .iter()
                .zip(&a.cells)
                .position(|(x, y)| x != y)
                .unwrap();
            panic!(
                "{what}: the picture differs at cell {first}: host {:?} app {:?}\nhost:\n{}\napp:\n{}",
                h.cells[first],
                a.cells[first],
                h.rows.join("\n"),
                a.rows.join("\n")
            );
        }
        assert_eq!(
            host_zoom(&h.rows),
            app_zoom(&a.rows),
            "{what}: the zoom differs"
        );
        let (hp, ap) = (host_page(&h.rows), app_page(&a.rows));
        assert!(
            hp.is_some(),
            "{what}: the host shows a page number: {:?}",
            h.rows[0]
        );
        assert_eq!(hp, ap, "{what}: the page number differs");
        pages.insert(hp.unwrap());
        let hh = key_hints(h.rows.last().expect("footer"));
        let mut ah = key_hints(a.rows.last().expect("footer"));
        ah.pop(); // the title
        for t in hh.iter().filter(|t| !ah.contains(t)) {
            seen_host_only.insert(t.clone());
            assert!(
                HOST_ONLY.contains(&t.as_str()) || HOST_ONLY_PAGED.contains(&t.as_str()),
                "{what}: the host footer has `{t}` that the app lacks\nhost: {hh:?}\napp: {ah:?}"
            );
        }
        for t in ah.iter().filter(|t| !hh.contains(t)) {
            assert_eq!(
                t, "q:quit",
                "{what}: the app footer has `{t}` that the host lacks"
            );
        }
        // The page keys are named in both.
        assert!(
            hh.iter().any(|t| t.contains("J/K")) && ah.iter().any(|t| t.contains("J/K")),
            "{what}: the page hint\nhost: {hh:?}\napp: {ah:?}"
        );
        hinted += 1;
        checked += 1;
        moved += usize::from(h.cells != at_fit.cells);
    }
    assert!(checked >= 14, "{checked}");
    assert!(hinted >= 14, "{hinted}");
    // The turns moved the picture, and the number took every value 1..=3.
    assert!(moved >= 8, "{moved}");
    assert_eq!(pages.len(), 3, "{pages:?}");
    assert!(seen_host_only.contains("O:pages"), "{seen_host_only:?}");
}

#[test]
fn a_single_page_has_no_page_keys_in_either() {
    let dir = sandbox("image_pages_one");
    make_pdf(&dir, "one.pdf", 1);
    let root = canon(&dir);
    let mut compared = 0;
    for spec in ["", "J", "K", "J K"] {
        let what = format!("one.pdf [{spec}]");
        let h = host_pdf_shot(&root, "one.pdf", HOST_H, spec, true);
        let a = app_pdf_shot(&root.join("one.pdf"), APP_H, spec, true);
        assert_eq!(h.cells, a.cells, "{what}: the picture differs");
        // The host numbers even a single page (`1/1`); the app says the same.
        assert_eq!(host_page(&h.rows).as_deref(), Some("1/1"), "{what}");
        assert_eq!(app_page(&a.rows), host_page(&h.rows), "{what}");
        let hh = key_hints(h.rows.last().unwrap());
        let ah = key_hints(a.rows.last().unwrap());
        assert!(!hh.iter().any(|t| t.contains("J/K")), "{what}: host {hh:?}");
        assert!(!ah.iter().any(|t| t.contains("J/K")), "{what}: app {ah:?}");
        compared += 1;
    }
    assert!(compared >= 4);
}

#[test]
fn without_a_picture_backend_neither_names_the_page_keys() {
    let dir = sandbox("image_pages_nobackend");
    make_pdf(&dir, "three.pdf", 3);
    let root = canon(&dir);
    let mut compared = 0;
    for spec in ["", "J", "K"] {
        let what = format!("three.pdf [{spec}] without a backend");
        let h = host_pdf_shot(&root, "three.pdf", HOST_H, spec, false);
        let a = app_pdf_shot(&root.join("three.pdf"), APP_H, spec, false);
        let hh = key_hints(h.rows.last().unwrap());
        let ah = key_hints(a.rows.last().unwrap());
        assert_eq!(
            hh.iter().any(|t| t.contains("J/K")),
            ah.iter().any(|t| t.contains("J/K")),
            "{what}: host {hh:?} app {ah:?}"
        );
        compared += 1;
    }
    assert!(compared >= 3);
}

const HELP_SPECS: &[&str] = &["?", "J ?", "J J J ?", "K ?"];

#[test]
fn the_image_help_names_the_page_keys_as_the_host_does() {
    let dir = sandbox("image_pages_help");
    make_pdf(&dir, "three.pdf", 3);
    make_pdf(&dir, "one.pdf", 1);
    let root = canon(&dir);
    let (mut compared, mut page_rows) = (0, 0);
    let mut host_only = std::collections::BTreeSet::new();
    for file in ["three.pdf", "one.pdf"] {
        for spec in HELP_SPECS {
            let what = format!("{file} [{spec}]");
            let h = host_pdf_shot(&root, file, 70, spec, true);
            let a = app_pdf_shot(&root.join(file), 69, spec, true);
            let (py, px) = h
                .rows
                .iter()
                .enumerate()
                .find_map(|(y, r)| {
                    r.find("┌ Help")
                        .map(|b| (y, unicode_width::UnicodeWidthStr::width(&r[..b])))
                })
                .expect("the help panel");
            let hs = image_section(&help_lines(&h.rows[py + 1..], Some(px)));
            let asec = image_section(&panel_lines(&a.rows));
            for row in &asec {
                if row.starts_with("q / Esc") || row.starts_with("? ") || row.starts_with("Q ") {
                    continue;
                }
                assert!(
                    hs.iter().any(|r| same_row(r, row)),
                    "{what}: the app's help row `{row}` is not in the host's\nhost: {hs:#?}\napp: {asec:#?}"
                );
                compared += 1;
            }
            for row in &hs {
                if row.starts_with("q / Esc") || asec.iter().any(|r| same_row(r, row)) {
                    continue;
                }
                let known = HOST_ONLY_ROWS
                    .iter()
                    .chain(["O pages"].iter())
                    .find(|p| row.starts_with(**p));
                assert!(
                    known.is_some(),
                    "{what}: the host's help row `{row}` is not in the app's\nhost: {hs:#?}\napp: {asec:#?}"
                );
                host_only.insert(*known.unwrap());
            }
            // The page row is there exactly when the host has it.
            let has = |v: &[String]| v.iter().any(|r| r.starts_with("J / K"));
            assert_eq!(
                has(&hs),
                has(&asec),
                "{what}: page row\nhost: {hs:#?}\napp: {asec:#?}"
            );
            assert_eq!(has(&hs), file == "three.pdf", "{what}");
            page_rows += usize::from(has(&asec));
        }
    }
    assert!(compared >= 25, "{compared}");
    assert_eq!(page_rows, HELP_SPECS.len(), "{page_rows}");
}
