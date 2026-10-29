//! The copy menu (`y`) in the konoma host and in the standalone app (`konoma-plugin` feature
//! `app`). Given the same file, the same size and the same keys, the two must show the same
//! which-key line (text and style), put the same text on the clipboard, say the same notice, and
//! list the same copy rows in the help. The clipboard is the test sink (never the real one).
//!
//! The displays: text, a document, a table (CSV, a TSV, a `.txt` a binding hint makes a TSV, a
//! spreadsheet whose cells are addressed) and a picture. The paths: a full-width name, a file
//! deep in the tree, a file directly under the root.

use super::image_parity::{host_sim_at, open_nested, PngPlugin, WhenIdle};
use super::table_parity::{panel_lines, presses, rows_of, APP_H, HOST_H, W};
use super::*;
use konoma_host_core::clipboard::{clear_test_clipboard, get_test_clipboard};
use konoma_plugin::app::{
    key_from_crossterm, run_with_pictures, AppCx, AppEvent, AppKeys, Display,
};
use konoma_plugin::{Plugin, TermCaps};
use konoma_test_support::TermScreen;
use ratatui::buffer::Buffer;
use ratatui::style::{Color as RColor, Modifier as RMod};
use std::sync::Arc;

/// One cell of a row: the symbol, both colors, the modifiers.
type Cell = (String, RColor, RColor, RMod);

/// The last row of a screen, as cells, without the blank cells at the end.
fn footer_cells(buf: &Buffer) -> Vec<Cell> {
    let y = buf.area.height - 1;
    let mut v: Vec<Cell> = (0..buf.area.width)
        .map(|x| {
            let c = &buf[(x, y)];
            (c.symbol().to_string(), c.fg, c.bg, c.modifier)
        })
        .collect();
    while v
        .last()
        .is_some_and(|c| c.0.trim().is_empty() && c.1 == RColor::Reset && c.3.is_empty())
    {
        v.pop();
    }
    v
}

fn text_of(cells: &[Cell]) -> String {
    cells.iter().map(|c| c.0.as_str()).collect::<String>()
}

/// What the app was opened with.
struct AppOpen {
    session: Box<dyn konoma_plugin::Session>,
    shows: Vec<konoma_plugin::manifest::ViewKind>,
    unavailable: konoma_plugin::manifest::Unavailable,
    path: std::path::PathBuf,
    hint: Option<String>,
}

fn app_open(plugins: Vec<Arc<dyn Plugin>>, path: &std::path::Path) -> AppOpen {
    let o = konoma_plugin::app::open_file(plugins, path, "en", TermCaps::new()).expect("opens");
    AppOpen {
        session: o.session,
        shows: o.shows,
        unavailable: o.unavailable,
        path: o.path,
        hint: o.hint,
    }
}

/// The app opened on `path` by `plugin` with the note `hint` of a binding (the app picks its
/// plugin by its own rules and has none that hands a note over, so the plugin is opened here).
fn app_open_hinted(plugin: Arc<dyn Plugin>, path: &std::path::Path, hint: &str) -> AppOpen {
    let settings = konoma_plugin::Settings::new();
    let cx = konoma_plugin::OpenCx::new(&settings, "en").with_term(TermCaps::new());
    let session = plugin
        .open(
            konoma_plugin::Input::File {
                path: path.to_path_buf(),
                hint: Some(hint.to_string()),
            },
            &cx,
        )
        .expect("opens");
    AppOpen {
        session,
        shows: plugin.manifest().shows,
        unavailable: plugin.manifest().unavailable,
        path: path.to_path_buf(),
        hint: Some(hint.to_string()),
    }
}

/// The app's screen and clipboard after the presses; the program started in `root`.
fn app_run(
    o: &mut AppOpen,
    root: &std::path::Path,
    h: u16,
    spec: &str,
) -> (Buffer, Option<String>) {
    clear_test_clipboard();
    let mut term = Terminal::new(TermScreen::new(W, h)).unwrap();
    let cx = AppCx {
        lang: "en".into(),
        term: TermCaps::new(),
        title: String::new(),
        unavailable: o.unavailable.clone(),
        shows: o.shows.clone(),
        path: o.path.clone(),
        hint: o.hint.clone(),
        open_dir: Some(root.to_path_buf()),
        display: Display::default(),
        ..AppCx::default()
    };
    let keys = presses(spec)
        .into_iter()
        .map(|(code, mods)| {
            AppEvent::Key(key_from_crossterm(&KeyEvent::new(code, mods)).expect("a key"))
        })
        .collect();
    let mut events = WhenIdle { keys, turns: 0 };
    run_with_pictures(
        &mut term,
        &mut *o.session,
        &mut events,
        &cx,
        &AppKeys::defaults(),
        None,
    )
    .unwrap();
    assert!(events.keys.is_empty(), "the app ended early: {spec}");
    (term.backend().1.clone(), get_test_clipboard())
}

/// The host after opening `file` from its tree and the presses, on a cleared screen.
fn host_run(
    root: &std::path::Path,
    cfg: Config,
    file: &str,
    h: u16,
    spec: &str,
) -> (Buffer, Option<String>) {
    clear_test_clipboard();
    let ext = file.rsplit('.').next().unwrap_or("");
    crate::preview::code::warm_file(ext, &root.join(file));
    let mut s = Sim::with_config_sized(root, cfg, W, h);
    open_nested(&mut s, file);
    for (code, mods) in presses(spec) {
        s.press(code, mods);
        assert!(!s.quit, "the host ended early: {spec}");
    }
    s.term.clear().expect("clear");
    s.draw();
    (s.term.backend().buffer().clone(), get_test_clipboard())
}

/// What was compared, and what was seen to differ from "nothing happened".
#[derive(Default)]
struct Tally {
    compared: usize,
    clipboards: usize,
    which_keys: usize,
    notices: usize,
    bold: usize,
    idle: usize,
    dim_items: usize,
}

/// Compare the host's and the app's footer row, cell by cell, and the clipboard.
fn same_copy(what: &str, h: (Buffer, Option<String>), a: (Buffer, Option<String>), t: &mut Tally) {
    let (hf, af) = (footer_cells(&h.0), footer_cells(&a.0));
    assert_eq!(
        text_of(&hf),
        text_of(&af),
        "{what}: the footer text differs"
    );
    assert_eq!(hf, af, "{what}: the footer styles differ");
    assert_eq!(h.1, a.1, "{what}: the clipboards differ");
    t.compared += hf.len() + 1;
    t.clipboards += usize::from(h.1.is_some());
    let text = text_of(&hf);
    let menu = text.contains('▸');
    t.which_keys += usize::from(menu);
    t.notices += usize::from(text.to_lowercase().contains("copied"));
    // The which-key heading is bold (up to its arrow), its items are not; the notice is bold too.
    let heading = hf
        .iter()
        .take_while(|c| c.0 != "▸")
        .all(|c| c.0 == " " || c.3.contains(RMod::BOLD));
    t.bold += usize::from(menu && heading && hf.iter().any(|c| c.3.contains(RMod::BOLD)));
    t.dim_items += usize::from(
        menu && hf
            .iter()
            .skip_while(|c| c.0 != "▸")
            .any(|c| c.3.contains(RMod::DIM)),
    );
}

/// The footer after a key that closed the menu or came after the notice: the hint row, which the
/// host fills with its own hints, so only what must agree is compared. The menu and the notice
/// are gone on both sides, the row is dim on both, the clipboard is the same.
fn same_idle(what: &str, h: (Buffer, Option<String>), a: (Buffer, Option<String>), t: &mut Tally) {
    let (hf, af) = (footer_cells(&h.0), footer_cells(&a.0));
    for (side, f) in [("host", &hf), ("app", &af)] {
        let text = text_of(f).to_lowercase();
        assert!(
            !text.contains("copied") && !text.contains("n name"),
            "{what}: {side} {text}"
        );
        let first = f.iter().find(|c| !c.0.trim().is_empty()).expect("a hint");
        assert!(
            first.3.contains(RMod::DIM),
            "{what}: {side} hint row is dim"
        );
        assert!(
            !first.3.contains(RMod::BOLD),
            "{what}: {side} hint row is not bold"
        );
        assert!(
            !first.3.contains(RMod::REVERSED),
            "{what}: {side} no reverse"
        );
    }
    assert_eq!(h.1, a.1, "{what}: the clipboards differ");
    t.compared += 7;
    t.clipboards += usize::from(h.1.is_some());
    t.idle += 1;
}

fn png(path: &std::path::Path) {
    image::RgbImage::from_pixel(8, 8, image::Rgb([200, 30, 30]))
        .save(path)
        .unwrap();
}

/// One display: its files (relative to the root), the plugins of the app and the keys to try.
struct Case {
    name: &'static str,
    files: Vec<&'static str>,
    plugins: fn() -> Vec<Arc<dyn Plugin>>,
    specs: &'static [&'static str],
}

const PATH_SPECS: &[&str] = &[
    "y", "y n", "y r", "y f", "y p", "y @", "y n j", "y x", "y <Esc>",
];
const TABLE_SPECS: &[&str] = &[
    "y", "y c", "y r", "y C", "y f", "l j y c", "l j y r", "l j y C", "j j y r", "y c j", "y x",
];

fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "text",
            files: vec!["main.rs", "全角の名前.rs", "alpha/beta/deep.rs"],
            plugins: || vec![Arc::new(konoma_code::CodePlugin)],
            specs: PATH_SPECS,
        },
        Case {
            name: "document",
            files: vec!["doc.md", "全角の名前.md", "alpha/beta/deep.md"],
            plugins: || vec![Arc::new(konoma_markdown::MarkdownPlugin)],
            specs: PATH_SPECS,
        },
        Case {
            name: "table",
            files: vec![
                "t.csv",
                "t.tsv",
                "全角の名前.csv",
                "alpha/beta/deep.csv",
                "one.xlsx",
                "全角.xlsx",
                "alpha/beta/deep.xlsx",
            ],
            plugins: || {
                vec![
                    Arc::new(konoma_table::CsvPlugin),
                    Arc::new(konoma_office::SpreadsheetPlugin),
                ]
            },
            specs: TABLE_SPECS,
        },
        Case {
            name: "image",
            files: vec!["p.png", "全角の名前.png"],
            plugins: || vec![Arc::new(PngPlugin)],
            specs: PATH_SPECS,
        },
    ]
}

fn fixtures() -> crate::test_support::TmpDir {
    let dir = sandbox("copy-parity");
    std::fs::create_dir_all(dir.join("alpha/beta")).unwrap();
    let rs = "fn main() {\n    println!(\"hi\");\n}\n";
    let md = "# Title\n\nsome text\n";
    for f in ["main.rs", "全角の名前.rs", "alpha/beta/deep.rs"] {
        std::fs::write(dir.join(f), rs).unwrap();
    }
    for f in ["doc.md", "全角の名前.md", "alpha/beta/deep.md"] {
        std::fs::write(dir.join(f), md).unwrap();
    }
    let csv = "名前,age,city\nann,30,tokyo\nbob,41,osaka\n";
    for f in ["t.csv", "全角の名前.csv", "alpha/beta/deep.csv"] {
        std::fs::write(dir.join(f), csv).unwrap();
    }
    std::fs::write(dir.join("t.tsv"), "a\tb\tc\n1\t2\t3\n4\t5\t6\n").unwrap();
    let sheet: String = (1..=4)
        .map(|r| {
            let cells: String = ["A", "B", "C"]
                .iter()
                .map(|c| x_str(&format!("{c}{r}"), &format!("s{c}{r}")))
                .collect();
            format!(r#"<row r="{r}">{cells}</row>"#)
        })
        .collect();
    for f in ["one.xlsx", "全角.xlsx", "alpha/beta/deep.xlsx"] {
        konoma_test_support::build_xlsx(&dir.join(f), &[("Only", "visible", &sheet, "")]);
    }
    std::fs::write(dir.join("h.txt"), "p\tq,r\n7\t8,9\n10\t11,12\n").unwrap();
    png(&dir.join("p.png"));
    png(&dir.join("全角の名前.png"));
    dir
}

/// Every key of the menu, on every file of every display: the which-key line after `y`, the
/// clipboard, the notice and the footer after the next key are the host's, cell by cell.
#[test]
fn the_copy_menu_the_clipboard_and_the_notice_agree_with_the_host() {
    let dir = fixtures();
    let root = canon(&dir);
    let mut t = Tally::default();
    let mut runs = 0;
    for case in cases() {
        for file in &case.files {
            for spec in case.specs {
                let what = format!("{} {file} [{spec}]", case.name);
                let h = if case.name == "image" {
                    clear_test_clipboard();
                    let mut sim = host_sim_at(&root, file, HOST_H, spec, false, None);
                    // A wide character drawn over leaves a stale cell in the test backend.
                    sim.term.clear().expect("clear");
                    sim.draw();
                    (sim.term.backend().buffer().clone(), get_test_clipboard())
                } else {
                    host_run(&root, cfg_en(), file, HOST_H, spec)
                };
                let mut o = app_open((case.plugins)(), &root.join(file));
                let a = app_run(&mut o, &root, APP_H, spec);
                if matches!(*spec, "y n j" | "y c j" | "y x" | "y <Esc>") {
                    same_idle(&what, h, a, &mut t);
                } else {
                    same_copy(&what, h, a, &mut t);
                }
                runs += 1;
            }
        }
    }
    assert!(runs >= 4 * 9, "{runs}");
    assert!(t.compared > 3000, "{}", t.compared);
    assert!(t.clipboards >= 60, "{}", t.clipboards);
    assert!(t.which_keys >= 15, "{}", t.which_keys);
    assert!(t.notices >= 60, "{}", t.notices);
    assert!(t.bold >= 15, "{}", t.bold);
    assert!(t.idle >= 30, "{}", t.idle);
    assert!(t.dim_items >= 15, "{}", t.dim_items);
}

/// A `.txt` a binding hint makes a TSV: the host copies a row joined by tabs, and so does the
/// app that is given the same hint. With the note `csv` both join with commas.
#[test]
fn a_binding_hint_decides_the_delimiter_of_a_copied_row_as_in_the_host() {
    let dir = fixtures();
    let root = canon(&dir);
    let cfg_as = |builtin: &str| {
        let mut cfg = cfg_en();
        cfg.preview.rules.insert(
            0,
            crate::config::Rule {
                glob: Some("*.txt".into()),
                builtin: Some(builtin.into()),
                ..crate::config::Rule::default()
            },
        );
        cfg
    };
    let mut t = Tally::default();
    for spec in ["y c", "y r", "l j y r", "j y c", "l y C"] {
        for note in ["tsv", "csv"] {
            let what = format!("h.txt as {note} [{spec}]");
            let h = host_run(&root, cfg_as(note), "h.txt", HOST_H, spec);
            let mut o =
                app_open_hinted(Arc::new(konoma_table::CsvPlugin), &root.join("h.txt"), note);
            let a = app_run(&mut o, &root, APP_H, spec);
            if spec == "y c" {
                // The first data cell: the tab splits it on a TSV, the comma on a CSV.
                let want = if note == "tsv" { "7" } else { "7\t8" };
                assert_eq!(h.1.as_deref(), Some(want), "{what}");
            }
            same_copy(&what, h, a, &mut t);
        }
    }
    assert!(t.clipboards >= 8, "{}", t.clipboards);
    let tab = host_run(&root, cfg_as("tsv"), "h.txt", HOST_H, "y c").1;
    let comma = host_run(&root, cfg_as("csv"), "h.txt", HOST_H, "y c").1;
    assert_ne!(tab, comma);
}

/// The copy rows of the help (`y n  name`...) are the host's, per display.
#[test]
fn the_copy_rows_of_the_help_agree_with_the_host() {
    let dir = fixtures();
    let root = canon(&dir);
    let mut compared = 0;
    for case in cases() {
        let file = case.files[0];
        let h = host_run(&root, cfg_en(), file, 130, "?").0;
        let mut o = app_open((case.plugins)(), &root.join(file));
        let a = app_run(&mut o, &root, 129, "?").0;
        let copy_rows = |buf: &Buffer| -> Vec<String> {
            let mut shown =
                Terminal::new(TestBackend::new(buf.area.width, buf.area.height)).unwrap();
            shown.draw(|f| *f.buffer_mut() = buf.clone()).expect("draw");
            panel_lines(&rows_of(&shown))
                .into_iter()
                .filter(|l| l.starts_with("y "))
                .collect()
        };
        let (hl, al) = (copy_rows(&h), copy_rows(&a));
        assert!(
            !hl.is_empty(),
            "{}: the host lists the copy rows",
            case.name
        );
        assert_eq!(hl, al, "{}: the copy rows of the help differ", case.name);
        compared += hl.len();
    }
    assert!(compared >= 4, "{compared}");
}
