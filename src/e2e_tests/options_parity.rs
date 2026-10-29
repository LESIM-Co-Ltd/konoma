//! The options menu (`,`) in the konoma host and in the standalone app (`konoma-plugin` feature
//! `app`). Given the same file, the same size and the same keys, the two must show the same
//! which-key line (text and style), no notice after an item (the host says none), the same frame (the title in the path style
//! the menu chose, the text wrapped or cut) and the same options rows in the help.
//!
//! The displays: text, a document, a table, a picture and a plugin's canvas. The items: `p` (the
//! path style, cycled), `w` (wrap), a key the menu does not have and `Esc`, pressed once and as
//! often as a cycle takes to come round again. The app starts in the directory the tab's root is,
//! so a title is written from the same base on both sides.

use super::copy_parity::{
    app_open, app_run_with, canvas_host, canvas_plugins, footer_cells, host_run, png, text_of,
};
use super::image_parity::host_sim_at;
use super::table_parity::{panel_lines, rows_of, APP_H, HOST_H};
use super::*;
use konoma_plugin::Plugin;
use ratatui::buffer::Buffer;
use ratatui::style::Modifier as RMod;
use std::sync::Arc;

/// One display: its files (relative to the root), the plugins of the app.
struct Case {
    name: &'static str,
    files: Vec<&'static str>,
    plugins: fn() -> Vec<Arc<dyn Plugin>>,
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "text",
            files: vec!["long.rs", "全角の名前.rs", "alpha/beta/deep.rs"],
            plugins: || vec![Arc::new(konoma_code::CodePlugin)],
        },
        Case {
            name: "document",
            files: vec!["long.md", "alpha/beta/deep.md"],
            plugins: || vec![Arc::new(konoma_markdown::MarkdownPlugin)],
        },
        Case {
            name: "table",
            files: vec!["t.csv", "alpha/beta/deep.csv"],
            plugins: || vec![Arc::new(konoma_table::CsvPlugin)],
        },
        Case {
            name: "image",
            files: vec!["p.png", "alpha/beta/deep.png"],
            plugins: || vec![Arc::new(super::image_parity::PngPlugin)],
        },
        Case {
            name: "canvas",
            files: vec!["a.pnt", "alpha/beta/deep.pnt"],
            plugins: canvas_plugins,
        },
    ]
}

/// The keys tried: the menu open, each item once and as often as a cycle takes (the path style
/// has three, wrap two), both items together, a key that is not an item, and the keys that close.
const SPECS: &[&str] = &[
    ",",
    ", p",
    ", p , p",
    ", p , p , p",
    ", p , p , p , p",
    ", w",
    ", w , w",
    ", w , w , w",
    ", p , w",
    ", w , p , p",
    ", x",
    ", <Esc>",
    ", q",
    ", p j",
    ", w G",
];

const LONG: usize = 450;

fn fixtures() -> crate::test_support::TmpDir {
    let dir = sandbox("options-parity");
    std::fs::create_dir_all(dir.join("alpha/beta")).unwrap();
    let rs = format!("// {}\nfn main() {{}}\n", "word ".repeat(LONG / 5));
    for f in ["long.rs", "全角の名前.rs", "alpha/beta/deep.rs"] {
        std::fs::write(dir.join(f), &rs).unwrap();
    }
    let md = format!("# Title\n\n{}\n\n- item\n", "word ".repeat(LONG / 5));
    for f in ["long.md", "alpha/beta/deep.md"] {
        std::fs::write(dir.join(f), &md).unwrap();
    }
    let csv = "名前,age,city\nann,30,tokyo\nbob,41,osaka\n";
    for f in ["t.csv", "alpha/beta/deep.csv"] {
        std::fs::write(dir.join(f), csv).unwrap();
    }
    for f in ["p.png", "alpha/beta/deep.png"] {
        png(&dir.join(f));
    }
    for f in ["a.pnt", "alpha/beta/deep.pnt"] {
        std::fs::write(dir.join(f), b"x").unwrap();
    }
    dir
}

/// The rows of a screen from the top border of the frame to its bottom border, whole width
/// (the host has a tab bar above and the app does not).
fn frame_rows(buf: &Buffer) -> Vec<String> {
    let rows: Vec<String> = (0..buf.area.height)
        .map(|y| {
            (0..buf.area.width)
                .map(|x| buf[(x, y)].symbol())
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect();
    let top = rows.iter().position(|l| l.starts_with('┌')).unwrap_or(0);
    // The last row is the footer; the frame ends above it.
    let end = rows.len() - 1;
    rows[top..end].to_vec()
}

/// The host on `file` of `case` after the keys.
fn host_of(case: &Case, root: &std::path::Path, file: &str, h: u16, spec: &str) -> Buffer {
    match case.name {
        "image" => {
            let mut sim = host_sim_at(root, file, h, spec, true, None);
            sim.term.clear().expect("clear");
            sim.draw();
            sim.term.backend().buffer().clone()
        }
        "canvas" => canvas_host(root, file, h, spec).0,
        _ => host_run(root, cfg_en(), file, h, spec).0,
    }
}

#[derive(Default)]
struct Tally {
    runs: usize,
    cells: usize,
    menus: usize,
    bold: usize,
    idle: usize,
    retitled: usize,
    rewrapped: usize,
    canvases: usize,
    cycles: usize,
    /// Files whose title was written three different ways (relative, `~/`, full).
    three_styles: usize,
    /// Titles that began with `~/` after one `p`.
    tilde: usize,
}

/// Every item, once and round a whole cycle, on every file of every display: the which-key
/// line, the notice, the frame and the footer after the next key are the host's.
#[test]
fn the_options_menu_agrees_with_the_host() {
    let dir = fixtures();
    let root = canon(&dir);
    // The fixtures are HOME (for this thread only), so the `home` style differs from `full`.
    let _home = konoma_host_core::paths::TestHome::set(&root);
    let mut t = Tally::default();
    for case in cases() {
        for file in &case.files {
            let base_h = host_of(&case, &root, file, HOST_H, "");
            for spec in SPECS {
                let what = format!("{} {file} [{spec}]", case.name);
                let h = host_of(&case, &root, file, HOST_H, spec);
                let mut o = app_open((case.plugins)(), &root.join(file));
                let a = app_run_with(&mut o, &root, APP_H, spec, case.name == "image").0;
                // The frame: the title, the text wrapped or cut.
                let (hr, ar) = (frame_rows(&h), frame_rows(&a));
                if case.name == "canvas" {
                    // The app draws a canvas without a frame or title (the plugin's cells
                    // fill the screen), so the host's frame has nothing to agree with: the
                    // app's canvas must not change under the menu.
                    let mut o = app_open((case.plugins)(), &root.join(file));
                    let base = app_run_with(&mut o, &root, APP_H, "", false).0;
                    assert_eq!(ar, frame_rows(&base), "{what}: the canvas changed");
                    t.canvases += 1;
                } else {
                    assert_eq!(hr, ar, "{what}: the frame differs");
                }
                t.cells += hr.iter().map(|r| r.chars().count()).sum::<usize>();
                if hr != frame_rows(&base_h) {
                    t.retitled += usize::from(hr[0] != frame_rows(&base_h)[0]);
                    t.rewrapped += usize::from(hr[1..] != frame_rows(&base_h)[1..]);
                }
                // The three path styles are three titles: relative, `~/...` and the full path.
                if case.name != "canvas" && *spec == ", p" {
                    let full = host_of(&case, &root, file, HOST_H, ", p , p");
                    let (t0, t1, t2) = (
                        frame_rows(&base_h)[0].clone(),
                        hr[0].clone(),
                        frame_rows(&full)[0].clone(),
                    );
                    assert!(t1.contains("~/"), "{what}: home title {t1}");
                    assert!(
                        !t0.contains("~/") && !t2.contains("~/"),
                        "{what}: {t0} {t2}"
                    );
                    assert!(t0 != t1 && t1 != t2 && t0 != t2, "{what}: {t0} {t1} {t2}");
                    t.three_styles += 1;
                    t.tilde += 1;
                }
                // A whole cycle comes round to where it began (three path styles, two wraps).
                if matches!(*spec, ", p , p , p" | ", w , w") && case.name != "canvas" {
                    assert_eq!(hr, frame_rows(&base_h), "{what}: the cycle did not return");
                    t.cycles += 1;
                }
                // The footer.
                let (hf, af) = (footer_cells(&h), footer_cells(&a));
                let (ht, at) = (text_of(&hf), text_of(&af));
                let waiting = ht.contains('▸');
                if waiting {
                    assert_eq!(ht, at, "{what}: the footer text differs");
                    assert_eq!(hf, af, "{what}: the footer styles differ");
                    t.cells += hf.len();
                } else {
                    // The host says nothing after an item (no notice), so neither does the app:
                    // the row is the hint row. The host fills it with its own hints, so only
                    // what must agree is compared: no menu, not bold (a notice is), dim, and
                    // the menu is named.
                    for (side, f, s) in [("host", &hf, &ht), ("app", &af, &at)] {
                        assert!(!s.contains('▸'), "{what}: {side} {s}");
                        assert!(s.contains(",:"), "{what}: {side} names the menu: {s}");
                        let first = f.iter().find(|c| !c.0.trim().is_empty()).expect("hint");
                        assert!(first.3.contains(RMod::DIM), "{what}: {side} dim");
                        assert!(!first.3.contains(RMod::BOLD), "{what}: {side} no notice");
                    }
                    t.idle += 1;
                }
                t.menus += usize::from(waiting);
                t.bold += usize::from(waiting && hf.iter().any(|c| c.3.contains(RMod::BOLD)));
                t.runs += 1;
            }
        }
    }
    let files: usize = cases().iter().map(|c| c.files.len()).sum();
    assert_eq!(t.runs, files * SPECS.len(), "every run was compared");
    assert!(t.cells > 20000, "{}", t.cells);
    assert!(t.menus >= files, "{}", t.menus);
    assert!(t.bold >= files, "{}", t.bold);
    assert!(t.idle >= files * (SPECS.len() - 1), "{}", t.idle);
    assert_eq!(t.cycles, 2 * (files - 2));
    assert_eq!(t.canvases, 2 * SPECS.len());
    assert_eq!(t.three_styles, files - 2);
    assert_eq!(t.tilde, files - 2);
    // Some of the keys changed the title, and some changed the text.
    assert!(t.retitled >= files * 3, "{}", t.retitled);
    assert!(t.rewrapped >= 6, "{}", t.rewrapped);
}

/// The options rows of the help (`, p  path`...) are the host's, per display.
#[test]
fn the_options_rows_of_the_help_agree_with_the_host() {
    let dir = fixtures();
    let root = canon(&dir);
    let rows = |buf: &Buffer| -> Vec<String> {
        let mut shown = Terminal::new(TestBackend::new(buf.area.width, buf.area.height)).unwrap();
        shown.draw(|f| *f.buffer_mut() = buf.clone()).expect("draw");
        panel_lines(&rows_of(&shown))
            .into_iter()
            .filter(|l| l.starts_with(", "))
            .collect()
    };
    let mut compared = 0;
    for case in cases() {
        let file = case.files[0];
        let h = host_of(&case, &root, file, 130, "?");
        let mut o = app_open((case.plugins)(), &root.join(file));
        let a = app_run_with(&mut o, &root, 129, "?", case.name == "image").0;
        let (hl, al) = (rows(&h), rows(&a));
        assert!(
            !hl.is_empty(),
            "{}: the host lists the options rows: {hl:?}",
            case.name
        );
        assert_eq!(hl, al, "{}: the options rows of the help differ", case.name);
        compared += hl.len();
    }
    assert!(compared >= 7, "{compared}");
}

/// A table cell's popup is open: `,` is not the menu there (the popup has its own keys), in the
/// host and in the app alike, and closing the popup leaves the table as it was.
#[test]
fn the_options_menu_does_not_open_over_the_cell_popup_as_in_the_host() {
    let dir = fixtures();
    let root = canon(&dir);
    let _home = konoma_host_core::paths::TestHome::set(&root);
    let case = &cases()[2];
    let mut compared = 0;
    for file in &case.files {
        let base = host_of(case, &root, file, HOST_H, "<CR>");
        let closed = host_of(case, &root, file, HOST_H, "");
        assert_ne!(
            frame_rows(&base),
            frame_rows(&closed),
            "{file}: the popup is open"
        );
        for spec in [
            "<CR> ,",
            "<CR> , p",
            "<CR> , w",
            "<CR> , p , p",
            "<CR> , <Esc>",
            "<CR> , p <Esc>",
            "<CR> , w <Esc> ,",
            "<CR> , p <Esc> , p",
        ] {
            let what = format!("{file} [{spec}]");
            let h = host_of(case, &root, file, HOST_H, spec);
            let mut o = app_open((case.plugins)(), &root.join(file));
            let a = app_run_with(&mut o, &root, APP_H, spec, false).0;
            assert_eq!(frame_rows(&h), frame_rows(&a), "{what}: the frame differs");
            let (ht, at) = (text_of(&footer_cells(&h)), text_of(&footer_cells(&a)));
            assert_eq!(ht.contains('▸'), at.contains('▸'), "{what}: {ht} | {at}");
            if spec.starts_with("<CR> , ") && !spec.contains("<Esc>") {
                // Still the popup: no menu, and the title is the one the popup opened with.
                assert!(
                    !ht.contains('▸'),
                    "{what}: the menu opened over the popup: {ht}"
                );
                assert_eq!(frame_rows(&h), frame_rows(&base), "{what}: popup changed");
            }
            compared += 1;
        }
    }
    assert_eq!(compared, 2 * 8);
}
