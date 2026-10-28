//! A table in the konoma host and in the standalone app (`konoma-plugin` feature `app`) is drawn
//! by the same shared code. Given the same file and the same keys, the two must show the same
//! table frame, the same cell cursor and popup, the same footer hints and the same help rows for
//! the keys both have (the host also has search, copy, tabs and so on, which the app does not).

use super::*;
use konoma_plugin::app::{open_file, run_with, AppCx, AppEvent, AppKeys};
use konoma_plugin::TermCaps;
use std::sync::Arc;

use crossterm::event::{KeyCode as KC, KeyModifiers as KM};

const W: u16 = 200;
/// The host's screen is the app's plus the host's one status row on top.
const HOST_H: u16 = 26;
const APP_H: u16 = HOST_H - 1;

/// One key press from a token: a character, or `<C-f>`, `<CR>`, `<Esc>`, `<PgDn>`, ...
fn token(t: &str) -> (KC, KM) {
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
fn presses(spec: &str) -> Vec<(KC, KM)> {
    spec.split_whitespace().map(token).collect()
}

/// The rows of a buffer, a wide symbol counted once.
fn rows_of(term: &Terminal<TestBackend>) -> Vec<String> {
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
    };
    let ev: Vec<AppEvent> = presses(spec)
        .into_iter()
        .map(|(code, mods)| {
            let ev = KeyEvent::new(code, mods);
            AppEvent::Key(konoma_plugin::app::key_from_crossterm(&ev).expect("a key"))
        })
        .collect();
    let mut it = ev.into_iter();
    run_with(&mut term, &mut *opened.session, &mut it, &cx, &AppKeys::defaults()).unwrap();
    assert_eq!(it.count(), 0, "the app ended before the keys ran out: {spec}");
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
    let top = rows.iter().position(|l| l.starts_with('┌')).expect("a frame");
    let bottom = rows.iter().rposition(|l| l.starts_with('└')).expect("a frame end");
    let mut out: Vec<String> = rows[top..=bottom].to_vec();
    let t = &out[0];
    let inner = t.trim_start_matches('┌').trim_start();
    let after_path = inner.split_once("  ").map_or("", |(_, rest)| rest);
    out[0] = format!("┌ {}", after_path.trim_end_matches(|c| c == '─' || c == '┐').trim_end());
    out
}

/// The footer's hints, split at the gaps.
fn hints(footer: &str) -> Vec<String> {
    footer
        .split("  ")
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .collect()
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
    konoma_test_support::build_xlsx(&dir.join("single.xlsx"), &[("Only", "visible", &sheet(1), "")]);
    let long: String = (0..100).map(|i| format!("r{i:03},{i},x\n")).collect();
    let tall: String = format!(
        "h1,h2\n\"{}\",b\nc,d\n",
        (0..40).map(|i| format!("L{i:02}")).collect::<Vec<_>>().join("\n")
    );
    let files: [(&str, String, bool); 7] = [
        ("plain.csv", "a,b,c\n1,2,3\n4,5,6\n7,8,9\n10,11,12\n13,14,15\n16,17,18\n".into(), false),
        ("cjk.csv", "名前,年齢,所在\n山田太郎,三十,東京\n鈴木,四十一,大阪\nabc,def,ghi\n".into(), true),
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
    "", "j", "k", "h", "l", "j j j j j j j j", "l l l l", "l l l l h", "G", "G g", "g", "$", "$ 0", "0",
    "G $", "G j", "$ l", "<C-f>", "<C-f> <C-f> <C-f>", "<C-b>", "G <C-b>", "<C-d>", "<C-d> <C-u>",
    "<C-u>", "<PgDn>", "<PgDn> <PgUp>", "<PgUp>", "<Down> <Right>", "<Up> <Left>", "<End>",
    "<Home>", "<End> <Home>", "<CR>", "j l <CR>", "G $ <CR>", "<CR> j j", "<CR> k", "<CR> G",
    "<CR> G g", "<CR> G k", "<CR> <PgDn>", "<CR> <PgDn> <PgUp>", "<CR> <PgUp>", "<CR> <Down> <Up>",
    "<CR> h l", "<CR> <C-f>", "<CR> <CR>", "<CR> <Esc>", "<CR> q", "<CR> j <CR> <CR>", "J", "K",
    "J J", "J J J", "J K", "K J", "J l j", "j l J", "j l J K", "J <CR>", "<CR> J", "<CR> K",
    "J <C-f>", "J G",
];

#[test]
fn the_frame_cursor_popup_and_footer_agree_with_the_host() {
    let (dir, files) = fixtures();
    let root = canon(&dir);
    let (mut checked, mut popups, mut paged, mut left) = (0, 0, 0, 0);
    for (file, _) in &files {
        for spec in SPECS {
            let (h, _s) = host_rows(&root, file, HOST_H, spec);
            let a = app_rows(&root.join(file), APP_H, spec);
            let what = format!("{file} [{spec}]");
            // A key that leaves the table (`Esc` or `q` with no popup open) goes back to the tree
            // in the host and quits the app: they do not share a screen after it.
            if h[0].contains("TREE") {
                assert!(
                    spec.contains("<Esc>") || spec.contains('q'),
                    "{what}: the host left the table without a leaving key"
                );
                left += 1;
                continue;
            }
            let (hf, af) = (frame(&h), frame(&a));
            assert_eq!(
                hf, af,
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
                checked += 1;
                continue;
            }
            // What only one of them says: the host has search, copy, files and tabs; the app
            // says `quit` where the host says `back`.
            let host_only: Vec<&String> = hh.iter().filter(|t| !ah.contains(t)).collect();
            let app_only: Vec<&String> = ah.iter().filter(|t| !hh.contains(t)).collect();
            for t in &host_only {
                assert!(
                    HOST_ONLY.iter().any(|p| t.starts_with(p)),
                    "{what}: the host footer has `{t}` that the app lacks\nhost: {hh:?}\napp: {ah:?}"
                );
            }
            for t in &app_only {
                assert!(
                    t.starts_with("q:quit"),
                    "{what}: the app footer has `{t}` that the host lacks\nhost: {hh:?}\napp: {ah:?}"
                );
            }
            checked += 1;
            popups += usize::from(af.iter().any(|l| l.contains("Cell ·")));
            paged += usize::from(af[0].contains("Two") || af[0].contains("Three"));
        }
    }
    assert!(checked > 500, "{checked}");
    assert!(popups > 100, "{popups}");
    assert!(paged > 5, "{paged}");
    assert!(left > 0 && left < 40, "{left}");
}

/// The hints only the host has (search, copy, the files, the tabs, `q` going back).
const HOST_ONLY: &[&str] = &["/:search", "y:copy", "C-n/C-p", "q:back", "t:", "[/]:", "e:", "o:", ",:", "n/N", "b:", "m:", "T:", "Space", "O:"];

/// The text of a help panel, one string per row, the runs of spaces made one.
fn help_lines(rows: &[String], panel_x: Option<usize>) -> Vec<String> {
    rows.iter()
        .map(|r| {
            let chars: Vec<char> = r.chars().collect();
            let cut: String = match panel_x {
                Some(x) if chars.len() > x + 1 => chars[x + 1..].iter().collect(),
                Some(_) => String::new(),
                None => r.clone(),
            };
            let cut = cut.trim_end_matches(|c| c == '│' || c == ' ' || c == '┐' || c == '┘');
            cut.split_whitespace().collect::<Vec<_>>().join(" ")
        })
        .collect()
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
const HOST_ONLY_ROWS: &[&str] = &["O pages", "/ n / N", "Ctrl-n / Ctrl-p", "m / '", "r refresh"];

/// The same row: the host's panel is narrower, so a long label may be cut where the app's is not.
fn same_row(a: &str, b: &str) -> bool {
    a.starts_with(b) || b.starts_with(a)
}

#[test]
fn the_table_help_rows_agree_with_the_host() {
    let (dir, _files) = fixtures();
    let root = canon(&dir);
    let mut compared = 0;
    for file in ["plain.csv", "cjk.csv", "header.csv", "empty.csv", "one.csv", "single.xlsx", "three.xlsx"] {
        for spec in ["?", "J ?", "G $ ?", "<CR> ?", "j l ?"] {
            let (h, _s) = host_rows(&root, file, 70, spec);
            let a = app_rows(&root.join(file), 69, spec);
            let what = format!("{file} [{spec}]");
            let (py, px) = h
                .iter()
                .enumerate()
                .find_map(|(y, r)| r.find("┌ Help").map(|b| (y, r[..b].chars().count())))
                .expect("the help panel");
            let hl = help_lines(&h[py + 1..], Some(px));
            let al = help_lines(&a, None);
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
                assert!(
                    HOST_ONLY_ROWS.iter().any(|p| row.starts_with(p)),
                    "{what}: the host's help row `{row}` is not in the app's\nhost: {hs:#?}\napp: {asec:#?}"
                );
            }
            // The order of the shared rows is the host's.
            let shared: Vec<&String> = hs.iter().filter(|r| asec.iter().any(|x| same_row(x, r))).collect();
            let theirs: Vec<&String> = asec.iter().filter(|r| hs.iter().any(|x| same_row(x, r))).collect();
            // `Enter` sits after the search row in the host and last in the app.
            let strip = |v: &[&String]| v.iter().filter(|r| !r.starts_with("Enter")).map(|r| r.chars().take(12).collect::<String>()).collect::<Vec<_>>();
            assert_eq!(strip(&shared), strip(&theirs), "{what}: the shared rows are in another order");
        }
    }
    assert!(compared > 200, "{compared}");
}
