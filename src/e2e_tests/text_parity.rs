//! A file shown as text (the `code` plugin) in the konoma host and in the standalone app
//! (`konoma-plugin` feature `app`). Given the same file, the same size and the same keys, the two
//! must draw the same frame: the path title, the position label, the scrollbar on the right
//! border and the rows inside, wrapped or cut and scrolled sideways.
//!
//! Not compared (known differences): `j`/`k` (the host moves a caret and the window follows it;
//! the app has no caret), `h` `l` and `$` after the caret has moved (they follow the caret), the
//! colors.

use super::table_parity::{hints, panel_lines, presses, rows_of, same_footer, same_help_rows};
use super::*;
use konoma_plugin::app::{open_file, run_with, AppCx, AppEvent, AppKeys, Display};
use konoma_plugin::TermCaps;
use std::sync::Arc;

/// What the app shows after the presses.
fn app_text(
    path: &std::path::Path,
    wrap: bool,
    tab_width: usize,
    (w, h): (u16, u16),
    spec: &str,
) -> Terminal<TestBackend> {
    let mut opened = open_file(
        vec![Arc::new(konoma_code::CodePlugin)],
        path,
        "en",
        TermCaps::new(),
    )
    .expect("opens");
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

/// A backend that applies the updates as a terminal does. `TestBackend` only stores the cells it is
/// given, so a cell that a wide character covered keeps what it held before: the wide character
/// is drawn over a mark at the cell after it (ratatui skips that cell), and when a narrow one is
/// drawn over the wide character's first cell later, ratatui sees a blank where a blank was and
/// sends nothing, while a real terminal has blanked the second half (tmux does; xterm, kitty
/// and Ghostty alike) and the stale mark would show in the test only. The host's frames
/// are free of it (it clears before the last frame), so the app's are made as a terminal shows.
struct Screen(TestBackend, ratatui::buffer::Buffer);

impl Screen {
    fn new(w: u16, h: u16) -> Self {
        Screen(
            TestBackend::new(w, h),
            ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, w, h)),
        )
    }
}

impl ratatui::backend::Backend for Screen {
    type Error = std::convert::Infallible;

    fn draw<'a, I>(&mut self, content: I) -> Result<(), Self::Error>
    where
        I: Iterator<Item = (u16, u16, &'a ratatui::buffer::Cell)>,
    {
        let wide =
            |c: &ratatui::buffer::Cell| unicode_width::UnicodeWidthStr::width(c.symbol()) > 1;
        let width = self.1.area.width;
        for (x, y, cell) in content {
            let buf = &mut self.1;
            // Drawing over the second half of a wide character blanks its first half.
            if x > 0 && wide(&buf[(x - 1, y)]) {
                buf[(x - 1, y)].reset();
            }
            // Drawing over the first half of one blanks its second half.
            if wide(&buf[(x, y)]) && x + 1 < width {
                buf[(x + 1, y)].reset();
            }
            buf[(x, y)] = cell.clone();
            // A wide character covers the cell after it.
            if wide(cell) && x + 1 < width {
                buf[(x + 1, y)].reset();
            }
        }
        Ok(())
    }
    fn hide_cursor(&mut self) -> Result<(), Self::Error> {
        self.0.hide_cursor()
    }
    fn show_cursor(&mut self) -> Result<(), Self::Error> {
        self.0.show_cursor()
    }
    fn get_cursor_position(&mut self) -> Result<ratatui::layout::Position, Self::Error> {
        self.0.get_cursor_position()
    }
    fn set_cursor_position<P: Into<ratatui::layout::Position>>(
        &mut self,
        p: P,
    ) -> Result<(), Self::Error> {
        self.0.set_cursor_position(p)
    }
    fn clear(&mut self) -> Result<(), Self::Error> {
        self.1.reset();
        self.0.clear()
    }
    fn clear_region(&mut self, c: ratatui::backend::ClearType) -> Result<(), Self::Error> {
        self.0.clear_region(c)
    }
    fn size(&self) -> Result<ratatui::layout::Size, Self::Error> {
        self.0.size()
    }
    fn window_size(&mut self) -> Result<ratatui::backend::WindowSize, Self::Error> {
        self.0.window_size()
    }
    fn flush(&mut self) -> Result<(), Self::Error> {
        self.0.flush()
    }
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
                        // `$` puts the host's caret on the last character and the window follows
                        // the caret's first cell: a last wide character is cut by one cell, where the
                        // app (no caret) shows the end of the line.
                        if name.starts_with("cjk") && spec.contains('$') {
                            continue;
                        }
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

/// What only the host's footer has for a text, spelled out in full.
const HOST_ONLY: &[&str] = &[
    ",:view",
    "[/]:tab",
    "/:search",
    "C-n/C-p:next/prev file",
    "e:edit",
    "o:open in app",
    "q:back",
    "v/V:select",
    "Y:@ref",
    "F:FOLLOW",
    "y:copy",
    "Tab:focus",
    "Tab:next tab",
    "O:outline",
    "R:raw source",
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
                    // The host always names `h/l:hscroll` and `0/$:line ends`; the app names
                    // them only when the text can scroll sideways.
                    let mut only = HOST_ONLY.to_vec();
                    for t in ["h/l:hscroll", "0/$:line ends"] {
                        if !af.iter().any(|a| a == t) {
                            only.push(t);
                        }
                    }
                    let (alone, n) =
                        same_footer(&what, &hf, &af, &only, &|t| t.starts_with("q:quit"));
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
            let ((_, hx), hb) = super::table_parity::panel_box(&rows_of(&host.term));
            let ((_, ax), ab) = super::table_parity::panel_box(&rows_of(&app));
            assert_eq!(hx, ax, "{what}: the panel's column");
            assert_eq!(hb[0], ab[0], "{what}: the border and its title");
            assert_eq!(hb.last(), ab.last(), "{what}: the bottom border");
        }
    }
}

/// The help rows only the host has for a text, by their start.
const TEXT_HELP_HOST_ONLY: &[&str] = &[
    "Launch",
    "Actions",
    "Sort & display",
    "Copy path",
    "Space ",
    ", ",
    "y ",
    "e ",
    "o ",
    "Tabs",
    "t ",
    "[ / ]",
    "1 - 9",
    "T ",
    "P ",
    "F ",
    "h / l",
    "0 / $",
    "/ n / N",
    "Y ",
    "v / V",
    "Ctrl-n / Ctrl-p",
    "m / '",
    "r ",
    "Tab ",
    "Enter ",
    "O ",
    "R ",
    "j / k / ↑ ↓ caret",
];
