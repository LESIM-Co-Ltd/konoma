//! A table in the konoma host and in the standalone app (`konoma-plugin` feature `app`) is drawn
//! by the same shared code. Given the same file and the same keys, the two must show the same
//! table frame, the same cell cursor and popup, the same footer hints and the same help rows for
//! the keys both have (the host also has search, copy, tabs and so on, which the app does not).

use super::*;
use konoma_plugin::app::{open_file, run_with, AppCx, AppEvent, AppKeys};
use konoma_plugin::TermCaps;
use std::sync::Arc;

use crossterm::event::{KeyCode as KC, KeyModifiers as KM};

pub(super) const W: u16 = 200;
/// The host's screen is the app's plus the host's one status row on top.
pub(super) const HOST_H: u16 = 26;
pub(super) const APP_H: u16 = HOST_H - 1;

/// One key press from a token: a character, or `<C-f>`, `<CR>`, `<Esc>`, `<PgDn>`, ...
pub(super) fn token(t: &str) -> (KC, KM) {
    match t {
        "<CR>" => (KC::Enter, KM::NONE),
        "<Esc>" => (KC::Esc, KM::NONE),
        "<PgDn>" => (KC::PageDown, KM::NONE),
        "<PgUp>" => (KC::PageUp, KM::NONE),
        "<Down>" => (KC::Down, KM::NONE),
        "<Up>" => (KC::Up, KM::NONE),
        "<Left>" => (KC::Left, KM::NONE),
        "<Right>" => (KC::Right, KM::NONE),
        "<Home>" => (KC::Home, KM::NONE),
        "<End>" => (KC::End, KM::NONE),
        t if t.starts_with("<C-") => (KC::Char(t.chars().nth(3).unwrap()), KM::CONTROL),
        t => {
            assert_eq!(t.chars().count(), 1, "token {t}");
            (KC::Char(t.chars().next().unwrap()), KM::NONE)
        }
    }
}

/// `"j j <C-f>"` -> the presses.
pub(super) fn presses(spec: &str) -> Vec<(KC, KM)> {
    spec.split_whitespace().map(token).collect()
}

/// The rows of a buffer, a wide symbol counted once.
pub(super) fn rows_of(term: &Terminal<TestBackend>) -> Vec<String> {
    let buf = term.backend().buffer();
    (0..buf.area.height)
        .map(|y| {
            let mut line = String::new();
            let mut x = 0;
            while x < buf.area.width {
                let sym = buf[(x, y)].symbol();
                line.push_str(sym);
                x += (unicode_width::UnicodeWidthStr::width(sym) as u16).max(1);
            }
            line.trim_end().to_string()
        })
        .collect()
}

/// What the app shows after the presses.
fn app_rows(path: &std::path::Path, h: u16, spec: &str) -> Vec<String> {
    let mut opened = open_file(
        vec![
            Arc::new(konoma_table::CsvPlugin),
            Arc::new(konoma_office::SpreadsheetPlugin),
        ],
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
        shows: opened.shows.clone(),
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
    assert_eq!(
        it.count(),
        0,
        "the app ended before the keys ran out: {spec}"
    );
    rows_of(&term)
}

/// What the host shows after the presses, with the file opened from its tree.
fn host_rows(root: &std::path::Path, file: &str, h: u16, spec: &str) -> (Vec<String>, Sim) {
    let mut s = Sim::with_config_sized(root, cfg_en(), W, h);
    s.select(file);
    s.enter();
    for (code, mods) in presses(spec) {
        s.press(code, mods);
        assert!(!s.quit, "the host ended before the keys ran out: {spec}");
    }
    (rows_of(&s.term), s)
}

/// The table frame and what is inside it, without the path in the title (the host shows its
/// own relative path, the app shows the title it was given).
fn frame(rows: &[String]) -> Vec<String> {
    let top = rows
        .iter()
        .position(|l| l.starts_with('┌'))
        .expect("a frame");
    let bottom = rows
        .iter()
        .rposition(|l| l.starts_with('└'))
        .expect("a frame end");
    let mut out: Vec<String> = rows[top..=bottom].to_vec();
    let t = &out[0];
    let inner = t.trim_start_matches('┌').trim_start();
    let after_path = inner.split_once("  ").map_or("", |(_, rest)| rest);
    out[0] = format!("┌ {}", after_path.trim_end_matches(['─', '┐']).trim_end());
    out
}

/// The footer's hints, split at the gaps.
pub(super) fn hints(footer: &str) -> Vec<String> {
    footer
        .split("  ")
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .collect()
}

/// Compare two footers in order. The host's tokens without those only the host has (`host_only`,
/// spelled out in full) must equal the app's without those only the app has (`app_only`): the
/// same words, in the same order. When either footer was cut short (its last token ends with
/// ` …`), the shorter list must be the start of the longer one, because the host spends width on
/// its own tokens and so cuts earlier. Returns the tokens the host has alone (so that the caller
/// can check each listed one occurs) and how many tokens were compared.
/// Every entry of an exclusion list must have been met at least once, or the list hides nothing
/// it claims to (a dead entry would let a real difference through unseen).
pub(super) fn assert_all_seen(
    list: &str,
    entries: &[&str],
    seen: &std::collections::BTreeSet<String>,
) {
    let missing: Vec<&&str> = entries.iter().filter(|e| !seen.contains(**e)).collect();
    assert!(missing.is_empty(), "{list}: never occurs: {missing:?}");
}

pub(super) fn same_footer(
    what: &str,
    hh: &[String],
    ah: &[String],
    host_only: &[&str],
    app_only: &dyn Fn(&str) -> bool,
) -> (Vec<String>, usize) {
    let strip = |v: &[String]| -> (Vec<String>, bool) {
        let mut v = v.to_vec();
        let cut = v.last().is_some_and(|t| t.ends_with('…'));
        if cut {
            let l = v.pop().unwrap();
            let l = l.trim_end_matches('…').trim_end().to_string();
            if !l.is_empty() {
                v.push(l);
            }
        }
        (v, cut)
    };
    let (hh2, hcut) = strip(hh);
    let (ah2, acut) = strip(ah);
    let mut alone = Vec::new();
    let hf: Vec<&String> = hh2
        .iter()
        .filter(|t| {
            let o = host_only.contains(&t.as_str());
            if o {
                alone.push((*t).clone());
            }
            !o
        })
        .collect();
    let af: Vec<&String> = ah2.iter().filter(|t| !app_only(t)).collect();
    let n = hf.len().min(af.len());
    if hcut || acut {
        assert_eq!(
            hf[..n],
            af[..n],
            "{what}: the cut footers differ\nhost: {hh:?}\napp: {ah:?}"
        );
    } else {
        assert_eq!(
            hf, af,
            "{what}: the footers differ in order or words\nhost: {hh:?}\napp: {ah:?}"
        );
    }
    (alone, n)
}

/// The fixture files: (name, whether a wide character is in it).
fn fixtures() -> (crate::test_support::TmpDir, Vec<(&'static str, bool)>) {
    let dir = sandbox("parity");
    let sheet = |k: usize| -> String {
        (1..=6)
            .map(|r| {
                let cells: String = ["A", "B", "C"]
                    .iter()
                    .map(|c| x_str(&format!("{c}{r}"), &format!("s{k}{c}{r}")))
                    .collect();
                format!(r#"<row r="{r}">{cells}</row>"#)
            })
            .collect()
    };
    konoma_test_support::build_xlsx(
        &dir.join("three.xlsx"),
        &[
            ("One", "visible", &sheet(1), ""),
            ("Two", "visible", &sheet(2), ""),
            ("Three", "visible", &sheet(3), ""),
        ],
    );
    konoma_test_support::build_xlsx(
        &dir.join("single.xlsx"),
        &[("Only", "visible", &sheet(1), "")],
    );
    let long: String = (0..100).map(|i| format!("r{i:03},{i},x\n")).collect();
    let tall: String = format!(
        "h1,h2\n\"{}\",b\nc,d\n",
        (0..40)
            .map(|i| format!("L{i:02}"))
            .collect::<Vec<_>>()
            .join("\n")
    );
    let files: [(&str, String, bool); 7] = [
        (
            "plain.csv",
            "a,b,c\n1,2,3\n4,5,6\n7,8,9\n10,11,12\n13,14,15\n16,17,18\n".into(),
            false,
        ),
        (
            "cjk.csv",
            "名前,年齢,所在\n山田太郎,三十,東京\n鈴木,四十一,大阪\nabc,def,ghi\n".into(),
            true,
        ),
        ("header.csv", "name,age\n".into(), false),
        ("empty.csv", String::new(), false),
        ("one.csv", "h\nx\n".into(), false),
        ("long.csv", format!("id,v,w\n{long}"), false),
        ("tall.csv", tall, false),
    ];
    let mut out = Vec::new();
    for (name, body, wide) in files {
        std::fs::write(dir.join(name), body).unwrap();
        out.push((name, wide));
    }
    out.push(("single.xlsx", false));
    out.push(("three.xlsx", false));
    (dir, out)
}

/// Keys to press, the cases of the cursor, the pages, the popup and the sheets.
const SPECS: &[&str] = &[
    "",
    "j",
    "k",
    "h",
    "l",
    "j j j j j j j j",
    "l l l l",
    "l l l l h",
    "G",
    "G g",
    "g",
    "$",
    "$ 0",
    "0",
    "G $",
    "G j",
    "$ l",
    "<C-f>",
    "<C-f> <C-f> <C-f>",
    "<C-b>",
    "G <C-b>",
    "<C-d>",
    "<C-d> <C-u>",
    "<C-u>",
    "<PgDn>",
    "<PgDn> <PgUp>",
    "<PgUp>",
    "<Down> <Right>",
    "<Up> <Left>",
    "<End>",
    "<Home>",
    "<End> <Home>",
    "<CR>",
    "j l <CR>",
    "G $ <CR>",
    "<CR> j j",
    "<CR> k",
    "<CR> G",
    "<CR> G g",
    "<CR> G k",
    "<CR> <PgDn>",
    "<CR> <PgDn> <PgUp>",
    "<CR> <PgUp>",
    "<CR> <Down> <Up>",
    "<CR> h l",
    "<CR> <C-f>",
    "<CR> <CR>",
    "<CR> <Esc>",
    "<CR> q",
    "<CR> j <CR> <CR>",
    "J",
    "K",
    "J J",
    "J J J",
    "J K",
    "K J",
    "J l j",
    "j l J",
    "j l J K",
    "J <CR>",
    "<CR> J",
    "<CR> K",
    "J <C-f>",
    "J G",
    // Sheets: the ends (`K` on the first, `J` on the last), back and forth, and what the cursor
    // and the popup do after a move.
    "K K",
    "J J J J",
    "J J K",
    "J J K K",
    "J K K",
    "J K J",
    "J J J K",
    "J j J",
    "J j l K",
    "J G J",
    "J G K",
    "J $ J",
    "J $ K",
    "J G $ J K",
    "J l l J J K",
    "J j j <C-f> J",
    "J <C-f> K",
    "J <CR> <Esc> K",
    "J <CR> <Esc> J",
    "J <CR> J",
    "J <CR> K",
    "J J <CR>",
    "J J <CR> K",
    "J J <CR> J",
    "J l j <CR> K",
    "J <CR> <CR> J",
    "<CR> J J",
    "<CR> J K",
    "<CR> <Esc> J",
    "G $ J",
    "G $ J <CR>",
    "G $ J K <CR>",
    "G $ <CR> <Esc> J",
    "J J J <CR>",
    "K <CR>",
    "K J",
    "K J J",
];

#[test]
fn the_frame_cursor_popup_and_footer_agree_with_the_host() {
    let (dir, files) = fixtures();
    let root = canon(&dir);
    let (mut checked, mut skipped, mut popups, mut paged, mut left) = (0, 0, 0, 0, 0);
    let mut seen_host_only = std::collections::BTreeSet::new();
    let mut compared = 0;
    for (file, _) in &files {
        for spec in SPECS {
            let (h, _s) = host_rows(&root, file, HOST_H, spec);
            let what = format!("{file} [{spec}]");
            // A key that leaves the table (`Esc` or `q` with no popup open) goes back to the tree
            // in the host and quits the app: they do not share a screen after it, and the app
            // may end before the rest of the keys, so it is not run.
            if h[0].contains("TREE") {
                assert!(
                    spec.contains("<Esc>") || spec.contains('q'),
                    "{what}: the host left the table without a leaving key"
                );
                left += 1;
                continue;
            }
            let a = app_rows(&root.join(file), APP_H, spec);
            let (hf, af) = (frame(&h), frame(&a));
            assert_eq!(
                hf,
                af,
                "{what}: the table frame differs\nhost:\n{}\napp:\n{}",
                h.join("\n"),
                a.join("\n")
            );
            let (hh, ah) = (
                hints(h.last().expect("footer")),
                hints(a.last().expect("footer")),
            );
            // The host answers `Enter` on a table without columns with a flash in the footer;
            // the standalone app has no flash line, so only the frame is compared there.
            if hh == ["no cell to view"] {
                skipped += 1;
                continue;
            }
            // The host has search, copy, files and tabs; the app says `quit` where the host
            // says `back`. Without those the two footers are the same words in the same order.
            let (alone, n) = same_footer(&what, &hh, &ah, HOST_ONLY, &|t| t.starts_with("q:quit"));
            seen_host_only.extend(alone);
            compared += n;
            checked += 1;
            popups += usize::from(af.iter().any(|l| l.contains("Cell ·")));
            paged += usize::from(af[0].contains("Two") || af[0].contains("Three"));
        }
    }
    // The footers really compared (the flash-only ones are counted apart, not here).
    assert!(checked > 880, "{checked}");
    assert!(skipped > 0 && skipped < 30, "{skipped}");
    for p in HOST_ONLY {
        assert!(seen_host_only.contains(*p), "HOST_ONLY `{p}` never occurs");
    }
    assert!(compared > 3000, "{compared}");
    assert!(popups > 200, "{popups}");
    assert!(paged > 30, "{paged}");
    assert!(left > 0 && left < 40, "{left}");
}

/// The hints only the host has (search, copy, the files, the tabs, `q` going back), spelled out
/// in full: a word the host adds that is not listed here fails the comparison.
const HOST_ONLY: &[&str] = &[
    ",:view",
    "[/]:tab",
    "/:search",
    "C-n/C-p:next/prev file",
    "e:edit",
    "e:open",
    "o:open in app",
    "O:pages",
    "q:back",
];

/// The text of a help panel, one string per row, the runs of spaces made one.
pub(super) fn help_lines(rows: &[String], panel_x: Option<usize>) -> Vec<String> {
    rows.iter()
        .map(|r| {
            // `x` is a display column: a wide character to the left counts for two.
            let cut: String = match panel_x {
                Some(x) => {
                    let mut col = 0;
                    let mut out = String::new();
                    for c in r.chars() {
                        if col > x {
                            out.push(c);
                        }
                        col += unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
                    }
                    // The picture shows beside the panel: stop at the panel's right border
                    // (no help text holds a `│`).
                    out.split('│').next().unwrap_or("").to_string()
                }
                None => r.clone(),
            };
            let cut = cut.trim_end_matches(['│', ' ', '┐', '┘', '█']);
            cut.split_whitespace().collect::<Vec<_>>().join(" ")
        })
        .collect()
}

/// The rows of the help panel on a screen (the app draws it over the screen as the host does):
/// the text under its top border, cut at the panel's left edge.
pub(super) fn panel_lines(rows: &[String]) -> Vec<String> {
    let (py, px) = panel_at(rows).unwrap_or_else(|| panic!("a help panel in {rows:#?}"));
    // Down to the panel's bottom border: the base display shows around and below it.
    let at_edge = |r: &String| {
        let mut col = 0;
        for c in r.chars() {
            if col >= px {
                return c;
            }
            col += unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
        }
        ' '
    };
    let body: Vec<String> = rows[py + 1..]
        .iter()
        .take_while(|r| at_edge(r) != '└')
        .cloned()
        .collect();
    help_lines(&body, Some(px))
}

/// The help panel as drawn: from its top-left corner to its bottom-right one, border included,
/// every cell, with the position of the corner. The panel is as wide as its top border.
pub(super) fn panel_box(rows: &[String]) -> ((usize, usize), Vec<String>) {
    let (py, px) = panel_at(rows).unwrap_or_else(|| panic!("a help panel in {rows:#?}"));
    // The cells of a row from display column `px` to the panel's right edge.
    let cells = |r: &str| -> Vec<(usize, char)> {
        let mut col = 0;
        let mut v = Vec::new();
        for c in r.chars() {
            v.push((col, c));
            col += unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
        }
        v
    };
    let top = cells(&rows[py]);
    let right = top
        .iter()
        .find(|(col, c)| *col > px && *c == '┐')
        .map(|(col, _)| *col)
        .expect("the panel's right edge");
    let mut out = Vec::new();
    for r in &rows[py..] {
        let line: String = cells(r)
            .into_iter()
            .filter(|(col, _)| *col >= px && *col <= right)
            .map(|(_, c)| c)
            .collect();
        let last = line.starts_with('└');
        out.push(line);
        if last {
            break;
        }
    }
    ((py, px), out)
}

/// The help rows (blank rows left out) of a host and of the app for a text or a Markdown file:
/// the host's without the rows only the host has (named by their start in `host_only`: search,
/// links, tasks, bookmarks, the tabs, the editor...) must be the app's, same rows, same order.
/// The app says `quit` where the host says `back to tree`. Returns the rows compared and the
/// `host_only` entries that took a row away.
pub(super) fn same_help_rows(
    what: &str,
    hl: &[String],
    al: &[String],
    host_only: &[&str],
) -> (usize, Vec<String>) {
    let norm = |r: &String| -> String {
        if r.starts_with("q / Esc") {
            "q / Esc".to_string()
        } else if r.starts_with("Q quit") {
            "Q quit".to_string()
        } else {
            r.clone()
        }
    };
    let mut used = Vec::new();
    let kept: Vec<String> = hl
        .iter()
        .filter(|r| {
            let hit = host_only.iter().find(|p| r.starts_with(**p));
            if let Some(p) = hit {
                used.push((*p).to_string());
            }
            hit.is_none()
        })
        .map(norm)
        .collect();
    let app: Vec<String> = al.iter().map(norm).collect();
    assert_eq!(
        kept, app,
        "{what}: the help rows differ\nhost: {hl:#?}\napp: {al:#?}"
    );
    (kept.len(), used)
}

/// The row of a help panel is the one that centres it in the whole screen it is drawn over, the
/// same rule in both (`help_panel::popup_layout`). The host's screen is one row taller than the
/// app's (`app_h`), and its panel holds more rows (the host's own keys), so the two rows differ by
/// exactly what the two heights say; a panel is at most two rows shorter than its screen.
/// `host` and `app` are the panel's (row, height).
pub(super) fn assert_placed_like_the_host(
    what: &str,
    host: (usize, usize),
    app: (usize, usize),
    app_h: usize,
) {
    let host_h = app_h + 1;
    assert_eq!(
        host.0,
        (host_h - host.1) / 2,
        "{what}: the host's panel is centred in its screen"
    );
    assert_eq!(
        app.0,
        (app_h - app.1) / 2,
        "{what}: the app's panel is centred in its screen"
    );
    assert!(host.1 <= host_h - 2, "{what}: the host's panel fits");
    assert!(app.1 <= app_h - 2, "{what}: the app's panel fits");
    // The app lists a subset of the host's rows: a panel capped by the screen in the app is
    // capped in the host too.
    if app.1 == app_h - 2 {
        assert_eq!(host.1, host_h - 2, "{what}: both panels are capped");
    }
}

/// The row and the display column of the help panel's top-left corner.
pub(super) fn panel_at(rows: &[String]) -> Option<(usize, usize)> {
    rows.iter().enumerate().find_map(|(y, r)| {
        r.find("┌ Help")
            .or_else(|| r.find("┌ ヘルプ"))
            .map(|b| (y, unicode_width::UnicodeWidthStr::width(&r[..b])))
    })
}

/// The rows of the table section of a help: from its heading to the blank row.
fn table_section(lines: &[String]) -> Vec<String> {
    let start = lines
        .iter()
        .position(|l| l == "CSV / TSV / spreadsheet / archive")
        .unwrap_or_else(|| panic!("the table section in {:#?}", &lines[..lines.len().min(12)]));
    lines[start + 1..]
        .iter()
        .take_while(|l| !l.is_empty() && !l.starts_with('│'))
        .cloned()
        .collect()
}

/// What only the host's table help lists (search, preview of the next file, bookmarks, refresh,
/// the sheet list), and the rows the two word differently.
const HOST_ONLY_ROWS: &[&str] = &[
    "O pages",
    "/ n / N",
    "Ctrl-n / Ctrl-p",
    "m / '",
    "r refresh",
];

/// The same row: the host's panel is narrower, so a long label may be cut where the app's is not.
pub(super) fn same_row(a: &str, b: &str) -> bool {
    a.starts_with(b) || b.starts_with(a)
}

/// Keys ending in `?`, and whether a cell popup is open when `?` is pressed.
const HELP_SPECS: &[(&str, bool)] = &[
    ("?", false),
    ("J ?", false),
    ("K ?", false),
    ("J K ?", false),
    ("J J ?", false),
    ("J J J ?", false),
    ("J l j ?", false),
    ("j l J ?", false),
    ("j l J K ?", false),
    ("G $ ?", false),
    ("G $ J ?", false),
    ("j l ?", false),
    ("<C-f> ?", false),
    ("<CR> <CR> ?", false),
    ("<CR> ?", true),
    ("j l <CR> ?", true),
    ("G $ <CR> ?", true),
    ("J <CR> ?", true),
    ("<CR> J ?", true),
    ("<CR> <PgDn> ?", true),
];

#[test]
fn the_table_help_rows_agree_with_the_host() {
    let (dir, _files) = fixtures();
    let root = canon(&dir);
    let mut compared = 0;
    let mut seen_rows = std::collections::BTreeSet::new();
    for file in [
        "plain.csv",
        "cjk.csv",
        "header.csv",
        "empty.csv",
        "one.csv",
        "single.xlsx",
        "three.xlsx",
    ] {
        for (spec, popup_open) in HELP_SPECS {
            // Whether a popup shows before `?` must match the flag (a table without cells
            // never opens one). The help is compared with the popup open too: the host draws
            // it above the other overlays.
            let before = spec.trim_end_matches('?').trim();
            let (h0, _s0) = host_rows(&root, file, 70, before);
            let has_cells = file != "empty.csv";
            assert_eq!(
                h0.iter().any(|l| l.contains("Cell ·")),
                *popup_open && has_cells,
                "{file} [{spec}]: the popup flag is wrong"
            );
            let (h, _s) = host_rows(&root, file, 70, spec);
            let a = app_rows(&root.join(file), 69, spec);
            let what = format!("{file} [{spec}]");
            let (py, px) = h
                .iter()
                .enumerate()
                .find_map(|(y, r)| {
                    r.find("┌ Help")
                        .map(|b| (y, unicode_width::UnicodeWidthStr::width(&r[..b])))
                })
                .expect("the help panel");
            let hl = help_lines(&h[py + 1..], Some(px));
            let al = panel_lines(&a);
            let (hs, asec) = (table_section(&hl), table_section(&al));
            for row in &asec {
                // The app says `quit` where the host says `back to tree`.
                if row.starts_with("q / Esc") {
                    assert!(hs.iter().any(|r| r.starts_with("q / Esc")), "{what}: {row}");
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
                let known = HOST_ONLY_ROWS.iter().find(|p| row.starts_with(**p));
                assert!(
                    known.is_some(),
                    "{what}: the host's help row `{row}` is not in the app's\nhost: {hs:#?}\napp: {asec:#?}"
                );
                seen_rows.insert((*known.unwrap()).to_string());
            }
            // The order of the shared rows is the host's.
            let shared: Vec<&String> = hs
                .iter()
                .filter(|r| asec.iter().any(|x| same_row(x, r)))
                .collect();
            let theirs: Vec<&String> = asec
                .iter()
                .filter(|r| hs.iter().any(|x| same_row(x, r)))
                .collect();
            // `Enter` sits after the search row in the host and last in the app.
            let strip = |v: &[&String]| {
                v.iter()
                    .filter(|r| !r.starts_with("Enter"))
                    .map(|r| r.chars().take(12).collect::<String>())
                    .collect::<Vec<_>>()
            };
            assert_eq!(
                strip(&shared),
                strip(&theirs),
                "{what}: the shared rows are in another order"
            );
        }
    }
    assert!(compared > 800, "{compared}");
    assert_all_seen("table HOST_ONLY_ROWS", HOST_ONLY_ROWS, &seen_rows);
}

/// A table without columns has no cell to view: the footer and the help leave `view cell` out and
/// `Enter` flashes (the gate of `ToggleTableCell`); a table with cells keeps all three.
#[test]
fn a_table_without_columns_does_not_offer_to_view_a_cell() {
    let (dir, _files) = fixtures();
    let root = canon(&dir);
    let footer_of = |file: &str, spec: &str| -> String {
        host_rows(&root, file, HOST_H, spec)
            .0
            .last()
            .cloned()
            .expect("footer")
    };
    let help_has = |file: &str| -> bool {
        let (h, _s) = host_rows(&root, file, 70, "?");
        h.iter().any(|l| l.contains("view the full cell text"))
    };
    // No columns: nothing offered, and `Enter` tells why while the table stays.
    assert!(
        !footer_of("empty.csv", "").contains("view cell"),
        "{}",
        footer_of("empty.csv", "")
    );
    assert!(!help_has("empty.csv"));
    let (rows, _s) = host_rows(&root, "empty.csv", HOST_H, "<CR>");
    assert!(
        rows.last().unwrap().contains("no cell to view"),
        "{rows:#?}"
    );
    assert!(!rows[0].contains("TREE"), "{rows:#?}");
    // With cells: offered, and `Enter` opens the popup.
    assert!(footer_of("plain.csv", "").contains("↵:view cell"));
    assert!(help_has("plain.csv"));
    let (rows, _s) = host_rows(&root, "plain.csv", HOST_H, "<CR>");
    assert!(rows.iter().any(|l| l.contains("Cell ·")), "{rows:#?}");
}

/// A workbook whose second sheet cannot be read (the XML of its rows is cut by a stray end tag).
fn broken_book(dir: &std::path::Path) {
    let ok = |k: &str| -> String {
        (1..=3)
            .map(|r| {
                format!(
                    r#"<row r="{r}">{}</row>"#,
                    x_str(&format!("A{r}"), &format!("{k}{r}"))
                )
            })
            .collect()
    };
    let bad = format!("{}</nope>", ok("bad"));
    konoma_test_support::build_xlsx(
        &dir.join("broken.xlsx"),
        &[
            ("One", "visible", &ok("a"), ""),
            ("Two", "visible", &bad, ""),
            ("Three", "visible", &ok("c"), ""),
        ],
    );
}

/// On a sheet that fails the host keeps `J` / `K` (so the footer and the help list them) and
/// has no cell to open; the app says the same.
#[test]
fn a_sheet_that_fails_has_the_same_footer_and_help_as_the_host() {
    let dir = sandbox("broken_sheet");
    broken_book(&dir);
    let root = canon(&dir);
    let mut compared = 0;
    for spec in [
        "J", "J j", "J l", "J G", "J <CR>", "J J K", "K J", "J K J", "J $",
    ] {
        let what = format!("broken.xlsx [{spec}]");
        let (h, _s) = host_rows(&root, "broken.xlsx", HOST_H, spec);
        let a = app_rows(&root.join("broken.xlsx"), APP_H, spec);
        // The failure line is the same string in both (the host draws it inside its frame).
        let line = |rows: &[String]| {
            rows.iter()
                .find(|l| l.contains("cannot preview"))
                .map(|l| l.trim_matches(['│', ' ']).to_string())
        };
        assert!(
            line(&h).is_some() && line(&h) == line(&a),
            "{what}: both show the same failure line\nhost:\n{}\napp:\n{}",
            h.join("\n"),
            a.join("\n")
        );
        let (hh, ah) = (
            hints(h.last().expect("footer")),
            hints(a.last().expect("footer")),
        );
        // `Enter` has no cell to open: the host flashes a line the app has no place for.
        if hh == ["no cell to view"] {
            continue;
        }
        assert!(hh.iter().any(|t| t == "J/K:sheet"), "{what}: {hh:?}");
        assert!(
            !hh.iter().any(|t| t.contains("view cell")),
            "{what}: {hh:?}"
        );
        for t in hh.iter().filter(|t| !ah.contains(t)) {
            assert!(
                HOST_ONLY.contains(&t.as_str()),
                "{what}: host `{t}`\nhost: {hh:?}\napp: {ah:?}"
            );
        }
        for t in ah.iter().filter(|t| !hh.contains(t)) {
            assert!(
                t.starts_with("q:quit"),
                "{what}: app `{t}`\nhost: {hh:?}\napp: {ah:?}"
            );
        }
        compared += 1;
    }
    assert!(compared >= 8, "{compared}");
    // The help.
    for spec in ["J ?", "J j ?", "J K J ?"] {
        let what = format!("broken.xlsx [{spec}]");
        let (h, _s) = host_rows(&root, "broken.xlsx", 70, spec);
        let a = app_rows(&root.join("broken.xlsx"), 69, spec);
        let px = h
            .iter()
            .find_map(|r| {
                r.find("┌ Help")
                    .map(|b| unicode_width::UnicodeWidthStr::width(&r[..b]))
            })
            .expect("the help panel");
        let py = h.iter().position(|r| r.contains("┌ Help")).unwrap();
        let hs = table_section(&help_lines(&h[py + 1..], Some(px)));
        let asec = table_section(&panel_lines(&a));
        assert!(
            asec.iter().any(|r| r.starts_with("J / K")),
            "{what}: {asec:#?}"
        );
        assert!(
            !asec.iter().any(|r| r.starts_with("Enter")),
            "{what}: {asec:#?}"
        );
        for row in &asec {
            if row.starts_with("q / Esc") {
                continue;
            }
            assert!(
                hs.iter().any(|r| same_row(r, row)),
                "{what}: app row `{row}` not in host\nhost: {hs:#?}"
            );
        }
        for row in &hs {
            if row.starts_with("q / Esc") || asec.iter().any(|r| same_row(r, row)) {
                continue;
            }
            assert!(
                HOST_ONLY_ROWS.iter().any(|p| row.starts_with(p)),
                "{what}: host row `{row}` not in app\nhost: {hs:#?}\napp: {asec:#?}"
            );
        }
    }
}

/// A workbook whose first sheet cannot be read (the rows are cut by a stray end tag).
fn broken_first_book(dir: &std::path::Path) {
    let ok = |k: &str| -> String {
        (1..=3)
            .map(|r| {
                format!(
                    r#"<row r="{r}">{}</row>"#,
                    x_str(&format!("A{r}"), &format!("{k}{r}"))
                )
            })
            .collect()
    };
    let bad = format!("{}</nope>", ok("bad"));
    konoma_test_support::build_xlsx(
        &dir.join("broken_first.xlsx"),
        &[
            ("One", "visible", &bad, ""),
            ("Two", "visible", &ok("b"), ""),
            ("Three", "visible", &ok("c"), ""),
        ],
    );
}

/// When the very first sheet fails, the host keeps `J` / `K` (nothing was shown before, yet the
/// workbook declares its sheets): the app turns to the next sheet and names the same keys.
#[test]
fn a_first_sheet_that_fails_turns_and_names_its_keys_as_the_host_does() {
    let dir = sandbox("broken_first_sheet");
    broken_first_book(&dir);
    let root = canon(&dir);
    let file = root.join("broken_first.xlsx");
    let mut compared = 0;
    for spec in ["", "J", "J j", "J J", "J J K", "K J", "J $"] {
        let what = format!("broken_first.xlsx [{spec}]");
        let (h, _s) = host_rows(&root, "broken_first.xlsx", HOST_H, spec);
        let a = app_rows(&file, APP_H, spec);
        let (hh, ah) = (
            hints(h.last().expect("footer")),
            hints(a.last().expect("footer")),
        );
        assert!(hh.iter().any(|t| t == "J/K:sheet"), "{what}: {hh:?}");
        assert!(ah.iter().any(|t| t == "J/K:sheet"), "{what}: app {ah:?}");
        if spec.contains('J') && !spec.ends_with("K") {
            // A sheet that reads: the same frame in both.
            if h.iter().any(|l| l.starts_with('┌')) {
                assert_eq!(frame(&h), frame(&a), "{what}");
            }
        }
        for t in hh.iter().filter(|t| !ah.contains(t)) {
            assert!(
                HOST_ONLY.contains(&t.as_str()),
                "{what}: host `{t}`\nhost: {hh:?}\napp: {ah:?}"
            );
        }
        for t in ah.iter().filter(|t| !hh.contains(t)) {
            assert!(
                t.starts_with("q:quit"),
                "{what}: app `{t}`\nhost: {hh:?}\napp: {ah:?}"
            );
        }
        compared += 1;
    }
    assert_eq!(compared, 7);
    // The help names the sheet keys.
    let (h, _s) = host_rows(&root, "broken_first.xlsx", 70, "?");
    let a = app_rows(&file, 69, "?");
    let py = h.iter().position(|r| r.contains("┌ Help")).expect("help");
    let px = h[py]
        .find("┌ Help")
        .map(|b| unicode_width::UnicodeWidthStr::width(&h[py][..b]))
        .unwrap();
    let hs = table_section(&help_lines(&h[py + 1..], Some(px)));
    let asec = table_section(&panel_lines(&a));
    assert!(hs.iter().any(|r| r.starts_with("J / K")), "{hs:#?}");
    assert!(asec.iter().any(|r| r.starts_with("J / K")), "{asec:#?}");
}
