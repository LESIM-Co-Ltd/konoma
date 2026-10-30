//! A file shown as text (the `code` plugin) in the konoma host and in the standalone app
//! (`konoma-plugin` feature `app`). Given the same file, the same size and the same keys, the two
//! must draw the same frame: the path title, the position label, the scrollbar on the right
//! border and the rows inside, wrapped or cut and scrolled sideways.
//!
//! The caret (the line cursor, the column cursor, the window that follows them) is shared code
//! (`konoma_widgets::caret`): the same keys must give the same frame, the same cells carrying the
//! caret and the line highlight, and the same line for `e`. Range selection (`v` `V`) and the
//! other colors are not compared.

use super::table_parity::{hints, panel_lines, presses, rows_of, same_footer, same_help_rows};
use super::*;
use konoma_plugin::app::{open_file, run_with, AppCx, AppEvent, AppKeys, Display};
use konoma_plugin::TermCaps;
use konoma_test_support::TermScreen as Screen;
use std::sync::Arc;

/// What the app shows after the presses.
fn app_text(
    path: &std::path::Path,
    wrap: bool,
    tab_width: usize,
    size: (u16, u16),
    spec: &str,
) -> Terminal<TestBackend> {
    app_text_by(
        Arc::new(konoma_code::CodePlugin),
        path,
        wrap,
        tab_width,
        size,
        spec,
    )
}

/// [`app_text`] with the plugin that shows the file given.
fn app_text_by(
    plugin: Arc<dyn konoma_plugin::Plugin>,
    path: &std::path::Path,
    wrap: bool,
    tab_width: usize,
    (w, h): (u16, u16),
    spec: &str,
) -> Terminal<TestBackend> {
    let mut opened = open_file(vec![plugin], path, "en", TermCaps::new()).expect("opens");
    let mut term = Terminal::new(Screen::new(w, h)).unwrap();
    let cx = AppCx {
        lang: "en".into(),
        term: TermCaps::new(),
        title: String::new(),
        shows: opened.shows.clone(),
        path: opened.path.clone(),
        open_dir: opened.path.parent().map(|p| p.to_path_buf()),
        display: Display {
            wrap,
            tab_width,
            ..Display::default()
        },
        launch: Some(super::table_parity::default_launch(&opened.manifest)),
        ..AppCx::default()
    };
    let ev: Vec<AppEvent> = presses(spec)
        .into_iter()
        .map(|(code, mods)| {
            let ev = KeyEvent::new(code, mods);
            AppEvent::Key(konoma_plugin::app::key_from_crossterm(&ev).expect("a key"))
        })
        .collect();
    let mut it = ev.into_iter();
    run_with(
        &mut term,
        &mut *opened.session,
        &mut it,
        &cx,
        &AppKeys::defaults(),
    )
    .unwrap();
    assert_eq!(it.count(), 0, "the app ended early: {spec}");
    // The screen the updates leave, as the frame the other tests read.
    let mut shown = Terminal::new(TestBackend::new(w, h)).unwrap();
    let screen = term.backend().1.clone();
    shown
        .draw(|f| *f.buffer_mut() = screen.clone())
        .expect("draw");
    shown
}

/// What the host shows after the presses, once the grammar is warm (so the coloring is not
/// pending: the state is checked, not a clock).
fn host_text(
    root: &std::path::Path,
    file: &str,
    wrap: bool,
    tab_width: usize,
    (w, h): (u16, u16),
    spec: &str,
) -> Sim {
    let ext = file.rsplit('.').next().unwrap_or("");
    crate::preview::code::warm_file(ext, &root.join(file));
    let mut cfg = cfg_en();
    cfg.ui.wrap = wrap;
    cfg.ui.tab_width = tab_width;
    let mut s = Sim::with_config_sized(root, cfg, w, h);
    s.select(file);
    s.enter();
    for (code, mods) in presses(spec) {
        s.press(code, mods);
        assert!(!s.quit, "the host ended early: {spec}");
    }
    assert!(!s.app.is_highlight_pending(), "{file}: still coloring");
    s.term.clear().expect("clear");
    s.draw();
    s
}

/// The frame by cell (from the top border to the bottom border, the whole width).
fn frame_cells(term: &Terminal<TestBackend>) -> Vec<String> {
    let rows = rows_of(term);
    let top = rows
        .iter()
        .position(|l| l.starts_with('┌'))
        .expect("a frame");
    let bottom = rows
        .iter()
        .rposition(|l| l.starts_with('└'))
        .expect("a frame end");
    let buf = term.backend().buffer();
    (top..=bottom)
        .map(|y| {
            let line: String = (0..buf.area.width)
                .map(|x| buf[(x, y as u16)].symbol())
                .collect();
            line.trim_end().to_string()
        })
        .collect()
}

fn fixtures() -> Vec<(String, String)> {
    let code: String = (0..70)
        .map(|i| format!("fn f{i}() {{\n    let x = {i};\n}}\n"))
        .collect();
    let long = format!(
        "// {}\nlet short = 1;\n{}\n",
        "word ".repeat(80),
        "x".repeat(300)
    );
    let cjk: String = (0..40)
        .map(|i| format!("// 日本語のコメント行 {i} です。これは長い行で折り返しを確かめます、とても長い。\nlet s{i} = \"全角\";\n"))
        .collect();
    vec![
        ("code.rs".into(), code),
        ("long.rs".into(), long),
        ("cjk.rs".into(), cjk),
        (
            "tabs.rs".into(),
            // The first line is the widest (`$` puts the host's caret at its end).
            format!("{}z{}\n", "\t".repeat(12), "x".repeat(40))
                + &(0..60)
                    .map(|i| {
                        format!(
                            "\t\tfn f{i}() {{\tlet x = {i};\t// note\tend\n{}ab\tc\tdef\t\tg\n",
                            "\t".repeat(i % 4)
                        )
                    })
                    .collect::<String>(),
        ),
        // A wide character before a tab: the column the tab stop counts from.
        (
            "tabswide.rs".into(),
            (0..60)
                .map(|i| format!("\t日本\tx{i}\t語語\ty\n{}あ\tz\n", "\t".repeat(i % 3)))
                .collect(),
        ),
        ("plain.sh".into(), "# one\n# two\necho hi\n".into()),
        ("noeol.toml".into(), "a = 1\nb = 2\nc = 3".to_string()),
        ("empty.json".into(), String::new()),
    ]
}

const SPECS: &[&str] = &[
    "",
    "G",
    "G g",
    "<C-f>",
    "<C-f> <C-f>",
    "<C-f> <C-b>",
    "<C-d>",
    "<C-d> <C-d> <C-u>",
    "<PgDn>",
    "<PgDn> <PgUp>",
    "<End>",
    "<End> <Home>",
    "$",
    "$ 0",
];

#[test]
fn the_text_frame_agrees_with_the_host() {
    let dir = sandbox("text-parity");
    for (name, body) in fixtures() {
        std::fs::write(dir.join(&name), body).unwrap();
    }
    let root = canon(&dir);
    let (mut checked, mut cells, mut labels, mut pcts, mut thumbs, mut marks) =
        (0, 0usize, 0, 0, 0, 0);
    for (name, _) in fixtures() {
        for wrap in [true, false] {
            // `[ui] tab_width` matters to the file with tabs only (the others have none).
            let tab_widths: &[usize] = if name.starts_with("tabs") {
                &[0, 1, 2, 4, 8, 16]
            } else {
                &[4]
            };
            for &tab_width in tab_widths {
                for (w, h) in [(60u16, 12u16), (200, 26), (24, 8)] {
                    for spec in SPECS {
                        let what =
                            format!("{name} wrap={wrap} tab_width={tab_width} {w}x{h} [{spec}]");
                        let host = host_text(&root, &name, wrap, tab_width, (w, h), spec);
                        let app = app_text(&root.join(&name), wrap, tab_width, (w, h - 1), spec);
                        // Compared by cell, the cell after a wide character left out (see
                        // `frame_cells`).
                        let (hr, ar) = (frame_cells(&host.term), frame_cells(&app));
                        assert_eq!(
                            hr,
                            ar,
                            "{what}: the frame differs\nhost:\n{}\napp:\n{}",
                            rows_of(&host.term).join("\n"),
                            rows_of(&app).join("\n")
                        );
                        checked += 1;
                        cells += hr.iter().map(|r| r.chars().count()).sum::<usize>();
                        labels += usize::from(
                            hr[0].contains("[Top]")
                                || hr[0].contains('%')
                                || hr[0].contains("[Bot]"),
                        );
                        pcts += usize::from(hr[0].contains('%'));
                        thumbs += usize::from(hr.iter().skip(1).any(|r| r.ends_with('█')));
                        marks += usize::from(hr.iter().any(|r| r.contains('→')));
                    }
                }
            }
        }
    }
    assert!(checked >= 1000, "{checked}");
    assert!(cells > 100_000, "{cells}");
    assert!(labels > 100, "{labels}");
    assert!(pcts > 20, "{pcts}");
    assert!(thumbs > 100, "{thumbs}");
    assert!(marks > 100, "frames with a tab mark: {marks}");
}

/// The cells that carry the caret (the reversed character) or the highlighted line, by their
/// place in the frame (from the top border), with the symbols: where the caret is drawn, not only
/// what the text says.
fn caret_cells(term: &Terminal<TestBackend>) -> Vec<(usize, u16, u8)> {
    use ratatui::style::{Color, Modifier};
    let rows = rows_of(term);
    let top = rows
        .iter()
        .position(|l| l.starts_with('┌'))
        .expect("a frame");
    let bottom = rows
        .iter()
        .rposition(|l| l.starts_with('└'))
        .expect("a frame end");
    let buf = term.backend().buffer();
    let mut out = Vec::new();
    for y in top..=bottom {
        for x in 0..buf.area.width {
            let c = &buf[(x, y as u16)];
            let mut flags = 0u8;
            if c.modifier.contains(Modifier::REVERSED) {
                flags |= 1;
            }
            if c.bg == Color::Rgb(55, 60, 74) {
                flags |= 2;
            }
            if flags != 0 {
                out.push((y - top, x, flags));
            }
        }
    }
    out
}

/// Files for the caret: the shapes its motions and its follow of the window depend on.
fn caret_fixtures() -> Vec<(String, String)> {
    let mut v = fixtures();
    // A file longer than every window, with lines wider and narrower than the screen.
    let big: String = (0..400)
        .map(|i| match i % 7 {
            0 => format!("// {} {i}\n", "wide ".repeat(30)),
            3 => "\n".to_string(),
            _ => format!("let v{i} = {i};\n"),
        })
        .collect();
    v.push(("big.rs".into(), big));
    v.push(("single.rs".into(), "fn main() {}\n".into()));
    v.push(("lone-noeol.rs".into(), "x".into()));
    v.push((
        "fullwidth.rs".into(),
        (0..50)
            .map(|i| {
                if i % 5 == 4 {
                    "\n".to_string()
                } else {
                    format!("{}{i}\n", "全角の文字だけの行です、".repeat(1 + i % 6))
                }
            })
            .collect(),
    ));
    v
}

/// The motions of a text, each key once: the caret's lines, the pages, the ends and the columns.
const MOTIONS: &[&str] = &[
    "j", "k", "g", "G", "<C-f>", "<C-b>", "<C-d>", "<C-u>", "<PgDn>", "<PgUp>", "h", "l", "0", "$",
];

/// The key sequences, built from the cases and not by hand: every pair of motions; every motion
/// after a start (the top, the end, a page, a half page, the line end, a run of `j` that passes
/// the window) followed by `j` and by `k`; and runs of one key around the window's size, at both
/// ends of the file.
fn caret_specs() -> Vec<String> {
    let mut out: Vec<String> = vec![String::new()];
    for a in MOTIONS {
        out.push((*a).to_string());
        for b in MOTIONS {
            out.push(format!("{a} {b}"));
        }
    }
    let run = |k: &str, n: usize| vec![k; n].join(" ");
    let starts = [
        "G".to_string(),
        "<C-f>".to_string(),
        "<C-d>".to_string(),
        "$".to_string(),
        "l l l l l".to_string(),
        run("j", 13),
    ];
    for st in &starts {
        for m in MOTIONS {
            for tail in ["j", "k"] {
                out.push(format!("{st} {m} {tail}"));
            }
        }
    }
    // Runs around the sizes of the windows used (10, 6 rows): short of, at and past the window.
    for n in [1usize, 2, 5, 6, 7, 9, 10, 11, 12, 13, 25, 60] {
        out.push(run("j", n));
        out.push(format!("G {}", run("k", n)));
        out.push(format!("{} {}", run("j", n), run("k", n / 2 + 1)));
        out.push(format!("{} G {}", run("j", n), run("j", 3)));
    }
    out
}

fn check_caret(wrap: bool) {
    let dir = sandbox(if wrap { "caret-wrap" } else { "caret-nowrap" });
    for (name, body) in caret_fixtures() {
        std::fs::write(dir.join(&name), body).unwrap();
    }
    let root = canon(&dir);
    let specs = caret_specs();
    let (mut checked, mut carets, mut lined, mut lines_seen) = (0usize, 0usize, 0usize, 0usize);
    let mut at = std::collections::BTreeSet::new();
    for (name, _) in caret_fixtures() {
        for (w, h) in [(60u16, 12u16), (24, 8)] {
            for spec in &specs {
                // The small terminal takes the longer sequences (the window is a few rows there:
                // the runs and the starts pass it); the pairs are checked in the larger one.
                if w < 40 && spec.split_whitespace().count() == 2 {
                    continue;
                }
                let what = format!("{name} wrap={wrap} {w}x{h} [{spec}]");
                let host = host_text(&root, &name, wrap, 4, (w, h), spec);
                let app = app_text(&root.join(&name), wrap, 4, (w, h - 1), spec);
                let (hr, ar) = (frame_cells(&host.term), frame_cells(&app));
                assert_eq!(
                    hr,
                    ar,
                    "{what}: the frame differs\nhost:\n{}\napp:\n{}",
                    rows_of(&host.term).join("\n"),
                    rows_of(&app).join("\n")
                );
                let (hc, ac) = (caret_cells(&host.term), caret_cells(&app));
                assert_eq!(hc, ac, "{what}: the caret and its line differ");
                let (he, ae) = (
                    host.app.preview_edit_line_for_test(),
                    konoma_plugin::app::edit_line(),
                );
                assert_eq!(he, ae, "{what}: the line `e` opens differs");
                checked += 1;
                carets += usize::from(hc.iter().any(|c| c.2 & 1 != 0));
                lined += usize::from(hc.iter().any(|c| c.2 & 2 != 0));
                if let Some(l) = he {
                    at.insert(l);
                    lines_seen += 1;
                }
            }
        }
    }
    // The bounds sit well below what the cases give (the exact counts are not the point).
    assert!(checked >= 7000, "{checked}");
    assert!(carets >= 5500, "frames with the caret character: {carets}");
    assert!(lined >= 5500, "frames with the highlighted line: {lined}");
    assert!(lines_seen >= 7000, "`e` lines compared: {lines_seen}");
    assert!(at.len() >= 80, "distinct `e` lines: {}", at.len());
}

#[test]
fn the_caret_agrees_with_the_host_wrapping() {
    check_caret(true);
}

#[test]
fn the_caret_agrees_with_the_host_scrolling_sideways() {
    check_caret(false);
}

/// The title is the path relative to the folder the app was opened in, a framed text carries no
/// "line N" in its footer (the label on the frame says where it is), and nothing is named
/// "preview" while a path is known.
#[test]
fn a_framed_text_is_titled_by_its_relative_path_and_the_footer_has_no_line_number() {
    let dir = sandbox("text-title");
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    let body: String = (0..80).map(|i| format!("fn f{i}() {{}}\n")).collect();
    std::fs::write(dir.join("sub/nested.rs"), body).unwrap();
    let root = canon(&dir);
    let app = app_text(&root.join("sub/nested.rs"), true, 4, (60, 11), "<C-f>");
    let rows = rows_of(&app);
    assert!(rows[0].contains("nested.rs"), "{}", rows[0]);
    assert!(!rows[0].contains("preview"), "{}", rows[0]);
    let foot = rows.last().unwrap();
    assert!(!foot.contains("line "), "{foot}");
    assert!(rows.iter().any(|r| r.contains('%')), "{rows:?}");
}

/// What only the host's footer has for a text, spelled out in full. The focus, the outline and
/// the raw source belong to a decorated document, so a text never shows them.
const HOST_ONLY: &[&str] = &[
    "[/]:tab",
    "/:search",
    "C-n/C-p:next/prev file",
    "q:back",
    "v/V:select",
    "Y:@ref",
    "F:FOLLOW",
];

/// The footer of a text is the host's footer without the host's own words: the same words in
/// the same order, wrapped or not, in a wide, a narrow and a very narrow terminal (where the
/// line is cut and ends with an ellipsis).
#[test]
fn the_text_footer_agrees_with_the_host() {
    let dir = sandbox("text-footer");
    for (name, body) in fixtures() {
        std::fs::write(dir.join(&name), body).unwrap();
    }
    let root = canon(&dir);
    let (mut compared, mut cut) = (0, 0);
    let mut seen = std::collections::BTreeSet::new();
    for name in ["code.rs", "long.rs", "tabs.rs"] {
        for wrap in [true, false] {
            for (w, h) in [(200u16, 26u16), (120, 20), (80, 12), (50, 12), (30, 8)] {
                for spec in ["", "G", "<C-f>", "l", "g"] {
                    let what = format!("{name} wrap={wrap} {w}x{h} [{spec}]");
                    let host = host_text(&root, name, wrap, 4, (w, h), spec);
                    let app = app_text(&root.join(name), wrap, 4, (w, h - 1), spec);
                    let (hf, af) = (
                        hints(rows_of(&host.term).last().expect("footer")),
                        hints(rows_of(&app).last().expect("footer")),
                    );
                    let (alone, n) =
                        same_footer(&what, &hf, &af, HOST_ONLY, &|t| t.starts_with("q:quit"));
                    seen.extend(alone);
                    compared += n;
                    cut += usize::from(hf.last().is_some_and(|t| t.ends_with('…')));
                }
            }
        }
    }
    assert!(compared > 200, "{compared}");
    assert!(cut > 20, "{cut}");
    assert!(seen.len() >= 6, "{seen:?}");
    super::table_parity::assert_all_seen("text HOST_ONLY", HOST_ONLY, &seen);
}

/// The help over a text: the rows are the host's without the host's own (the search, the
/// selection, the tabs, the editor, the chords...), the same rows in the same order; the panel is
/// placed and framed as the host's in large, medium, small and narrow terminals.
#[test]
fn the_help_over_a_text_agrees_with_the_host() {
    let dir = sandbox("text-help");
    for (name, body) in fixtures() {
        std::fs::write(dir.join(&name), body).unwrap();
    }
    let root = canon(&dir);
    let mut compared = 0;
    let mut seen = std::collections::BTreeSet::new();
    for name in ["code.rs", "long.rs", "cjk.rs"] {
        for wrap in [true, false] {
            for spec in ["?", "G ?", "<C-f> ?"] {
                let what = format!("{name} wrap={wrap} [{spec}]");
                let host = host_text(&root, name, wrap, 4, (100, 130), spec);
                let app = app_text(&root.join(name), wrap, 4, (99, 129), spec);
                let (hr, ar) = (rows_of(&host.term), rows_of(&app));
                let nonblank = |v: Vec<String>| -> Vec<String> {
                    v.into_iter().filter(|l| !l.is_empty()).collect()
                };
                let (hl, al) = (nonblank(panel_lines(&hr)), nonblank(panel_lines(&ar)));
                let (n, used) = same_help_rows(&what, &hl, &al, TEXT_HELP_HOST_ONLY);
                compared += n;
                seen.extend(used);
            }
        }
    }
    assert!(compared > 60, "{compared}");
    assert!(seen.len() >= 6, "{seen:?}");
    super::table_parity::assert_all_seen("text TEXT_HELP_HOST_ONLY", TEXT_HELP_HOST_ONLY, &seen);
    // The frame of the panel, in every size.
    for name in ["code.rs", "long.rs", "cjk.rs"] {
        for (w, h) in [
            (200u16, 70u16),
            (100, 40),
            (80, 24),
            (66, 14),
            (60, 12),
            (40, 10),
        ] {
            let what = format!("{name} {w}x{h}");
            let host = host_text(&root, name, true, 4, (w, h + 1), "?");
            let app = app_text(&root.join(name), true, 4, (w, h), "?");
            let ((hy, hx), hb) = super::table_parity::panel_box(&rows_of(&host.term));
            let ((ay, ax), ab) = super::table_parity::panel_box(&rows_of(&app));
            assert_eq!(hx, ax, "{what}: the panel's column");
            super::table_parity::assert_placed_like_the_host(
                &what,
                (hy, hb.len()),
                (ay, ab.len()),
                usize::from(h),
            );
            assert_eq!(hb[0], ab[0], "{what}: the border and its title");
            assert_eq!(hb.last(), ab.last(), "{what}: the bottom border");
        }
    }
}

/// The help rows only the host has for a text, by their start.
const TEXT_HELP_HOST_ONLY: &[&str] = &[
    "Actions",
    "Space ",
    "Tabs",
    "t ",
    "[ / ]",
    "1 - 9",
    "T ",
    "P ",
    "F ",
    "/ n / N",
    "Y ",
    "v / V",
    "Ctrl-n / Ctrl-p",
    "m / '",
    "r ",
];

// ---- generated text (`TextSource::Owned` / `Lines`) -----------------------------------------------

/// A plugin that hands the interface its own text. The caret is drawn on it in both hosts; `e`
/// has a line only when the plugin says the lines are the file's (`file_lines`).
struct GenText {
    id: String,
    owned: bool,
    file_lines: bool,
}

fn gen_body() -> String {
    let mut out: Vec<String> = (0..40).map(|i| format!("generated line {i:02}")).collect();
    out[3] = format!("long {}", "0123456789".repeat(12));
    out[7] = String::new();
    out.join("\n")
}

impl konoma_plugin::Plugin for GenText {
    fn manifest(&self) -> konoma_plugin::Manifest {
        use konoma_plugin::prelude::*;
        Manifest::new(self.id.as_str(), "0.1.0")
            .bind(Bind::files(["*.gen"]))
            .shows(ViewKind::Text)
    }

    fn open(
        &self,
        _: konoma_plugin::Input,
        _: &konoma_plugin::OpenCx,
    ) -> Result<Box<dyn konoma_plugin::Session>, konoma_plugin::Failure> {
        Ok(Box::new(GenSession {
            owned: self.owned,
            file_lines: self.file_lines,
        }))
    }
}

struct GenSession {
    owned: bool,
    file_lines: bool,
}

impl konoma_plugin::Session for GenSession {
    fn view(
        &mut self,
        _: &konoma_plugin::ViewReq,
        _: &konoma_plugin::ViewCx,
    ) -> Result<konoma_plugin::View, konoma_plugin::Failure> {
        use konoma_plugin::view::{TextSource, TextView};
        let body = gen_body();
        let source = if self.owned {
            TextSource::Owned(body)
        } else {
            TextSource::Lines(body.lines().map(konoma_plugin::Line::raw).collect())
        };
        Ok(konoma_plugin::View::Text(TextView {
            source,
            styler: None,
            file_lines: self.file_lines,
            syntax_name: None,
        }))
    }
}

/// The caret, its line and the frame on a plugin's own text, and the line `e` opens at, are the
/// host's, for each of the ways to hand text over and for `file_lines` both ways.
#[test]
fn the_caret_on_generated_text_agrees_with_the_host() {
    let specs: Vec<String> = caret_specs()
        .into_iter()
        .filter(|s| s.split_whitespace().count() <= 2)
        .collect();
    let (mut checked, mut some, mut none, mut carets) = (0usize, 0usize, 0usize, 0usize);
    for (owned, file_lines) in [(true, true), (true, false), (false, true), (false, false)] {
        for wrap in [true, false] {
            let name = format!("gen-{owned}-{file_lines}-{wrap}");
            let dir = sandbox(&name);
            std::fs::write(dir.join("a.gen"), "x").unwrap();
            let id = format!("x-gen-{name}");
            let mk = || GenText {
                id: id.clone(),
                owned,
                file_lines,
            };
            let marker = crate::plugins::test_plugins::register(&name, Arc::new(mk()));
            let root = canon(&dir);
            for spec in &specs {
                let what = format!("owned={owned} file_lines={file_lines} wrap={wrap} [{spec}]");
                let mut cfg = crate::plugins::test_plugins::marker_config(&marker);
                cfg.ui.lang = "en".into();
                cfg.ui.wrap = wrap;
                let mut s = Sim::with_config_sized(&root, cfg, 60, 12);
                s.select("a.gen");
                s.enter();
                s.draw();
                for (code, mods) in presses(spec) {
                    s.press(code, mods);
                    assert!(!s.quit, "{what}: the host ended early");
                }
                s.term.clear().expect("clear");
                s.draw();
                let app = app_text_by(Arc::new(mk()), &root.join("a.gen"), wrap, 4, (60, 11), spec);
                assert_eq!(
                    frame_cells(&s.term),
                    frame_cells(&app),
                    "{what}: the frame differs\nhost:\n{}\napp:\n{}",
                    rows_of(&s.term).join("\n"),
                    rows_of(&app).join("\n")
                );
                let (hc, ac) = (caret_cells(&s.term), caret_cells(&app));
                assert_eq!(hc, ac, "{what}: the caret and its line differ");
                let (he, ae) = (
                    s.app.preview_edit_line_for_test(),
                    konoma_plugin::app::edit_line(),
                );
                assert_eq!(he, ae, "{what}: the line `e` opens differs");
                assert_eq!(he.is_some(), file_lines, "{what}: `e` follows file_lines");
                carets += usize::from(hc.iter().any(|c| c.2 & 1 != 0));
                some += usize::from(he.is_some());
                none += usize::from(he.is_none());
                checked += 1;
            }
        }
    }
    assert!(checked >= 1700, "{checked}");
    assert!(carets >= 1480, "frames with the caret character: {carets}");
    assert!(some >= 850 && none >= 850, "{some} {none}");
}
