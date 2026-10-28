//! Tables in the app: the grid drawn with the host's widgets, the cell cursor, the pages
//! (sheets), the full-cell popup, the help and the footer hints.

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Arc;

use konoma_plugin::app::{open_file, run_with, AppCx, AppEvent, AppKeys};
use konoma_plugin::prelude::*;
use konoma_plugin::TermCaps;
use konoma_test_support::{unique_tmp, TmpDir};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

use crate::table_plugin::TablePlugin;

fn cx() -> AppCx {
    AppCx {
        lang: "en".into(),
        term: TermCaps::new(),
        title: "title".into(),
    }
}

fn kc(k: KeyCode) -> AppEvent {
    AppEvent::Key(Key::new(k))
}

fn kk(c: char) -> AppEvent {
    AppEvent::Key(Key::char(c))
}

fn ctrl(c: char) -> AppEvent {
    AppEvent::Key(Key::ctrl(c))
}

fn chars(s: &str) -> Vec<AppEvent> {
    s.chars().map(kk).collect()
}

fn file(name: &str, body: &str) -> (TmpDir, PathBuf) {
    let dir = unique_tmp("app-table");
    std::fs::create_dir_all(dir.as_path()).unwrap();
    let p = dir.as_path().join(name);
    std::fs::write(&p, body).unwrap();
    (dir, p)
}

/// Run the app over `path`; returns the drawn rows, the number of `view` calls, and how many
/// events were left unread (0 = all were consumed, so the app did not quit before the end).
fn run_counted(
    path: &Path,
    size: (u16, u16),
    events: Vec<AppEvent>,
) -> (Vec<String>, usize, usize) {
    run_keyed(path, size, events, &AppKeys::defaults())
}

/// [`run_counted`] with the keys of a config.
fn run_keyed(
    path: &Path,
    (w, h): (u16, u16),
    events: Vec<AppEvent>,
    keys: &AppKeys,
) -> (Vec<String>, usize, usize) {
    let (plugin, views) = TablePlugin::new();
    let mut opened = open_file(vec![Arc::new(plugin)], path, "en", TermCaps::new()).expect("opens");
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    let mut it = events.into_iter();
    run_with(&mut term, &mut *opened.session, &mut it, &cx(), keys).expect("runs");
    let left = it.count();
    let buf = term.backend().buffer();
    let rows = (0..buf.area.height)
        .map(|y| {
            // The cell after a wide symbol is its continuation: it adds no text.
            let mut line = String::new();
            let mut x = 0;
            while x < buf.area.width {
                let sym = buf[(x, y)].symbol();
                line.push_str(sym);
                x += (unicode_width::UnicodeWidthStr::width(sym) as u16).max(1);
            }
            line.trim_end().to_string()
        })
        .collect();
    (rows, views.load(Ordering::SeqCst), left)
}

fn run(path: &Path, size: (u16, u16), events: Vec<AppEvent>) -> Vec<String> {
    run_counted(path, size, events).0
}

fn all(rows: &[String]) -> String {
    rows.join("\n")
}

fn footer(rows: &[String]) -> String {
    rows.last().cloned().unwrap_or_default()
}

fn title(rows: &[String]) -> String {
    rows[0].clone()
}

const SIZE: (u16, u16) = (80, 12);

fn people() -> (TmpDir, PathBuf) {
    file("p.grid", "name,age\nann,30\nbob,41\ncy,52\n")
}

fn book() -> (TmpDir, PathBuf) {
    file("w.book", "a,b\nc,d\n===\ne,f\n===\ng,h\ni,j\n")
}

fn long(n: usize) -> String {
    let mut s = String::from("id,v\n");
    for i in 0..n {
        s += &format!("r{i:03},{i}\n");
    }
    s
}

#[test]
fn a_csv_is_drawn_as_a_grid_with_its_header() {
    let (_d, p) = people();
    let rows = run(&p, SIZE, vec![]);
    let s = all(&rows);
    assert!(title(&rows).contains("title"), "{s}");
    assert!(title(&rows).contains("r1/3 c1/2"), "{s}");
    assert!(title(&rows).contains("3×2"), "{s}");
    assert!(rows[1].contains("name") && rows[1].contains("age"), "{s}");
    assert!(rows[3].contains("ann") && rows[3].contains("30"), "{s}");
    assert!(rows[5].contains("cy"), "{s}");
}

#[test]
fn a_spreadsheet_has_a_gutter_addresses_and_the_sheet_note() {
    let (_d, p) = book();
    let rows = run(&p, SIZE, vec![]);
    let s = all(&rows);
    assert!(title(&rows).contains("Sheet1 (1/3)"), "{s}");
    assert!(title(&rows).contains("A1"), "{s}");
    assert!(rows[1].contains('A') && rows[1].contains('B'), "{s}");
    assert!(rows[3].starts_with("│1 a"), "{s}");
    assert!(rows[4].starts_with("│2 c"), "{s}");
}

#[test]
fn the_cursor_moves_by_every_cell_key_and_stops_at_the_edges() {
    let (_d, p) = people();
    let at = |ev: Vec<AppEvent>| title(&run(&p, SIZE, ev));
    assert!(at(chars("j")).contains("r2/3 c1/2"));
    assert!(at(chars("jj")).contains("r3/3 c1/2"));
    assert!(
        at(chars("jjjjj")).contains("r3/3 c1/2"),
        "stops at the last row"
    );
    assert!(
        at(chars("k")).contains("r1/3 c1/2"),
        "stops at the first row"
    );
    assert!(at(chars("l")).contains("r1/3 c2/2"));
    assert!(
        at(chars("ll")).contains("r1/3 c2/2"),
        "stops at the last column"
    );
    assert!(
        at(chars("h")).contains("r1/3 c1/2"),
        "stops at the first column"
    );
    assert!(at(chars("lh")).contains("r1/3 c1/2"));
    assert!(at(chars("G")).contains("r3/3 c1/2"));
    assert!(at(chars("Gg")).contains("r1/3 c1/2"));
    assert!(at(chars("$")).contains("r1/3 c2/2"));
    assert!(at(chars("$0")).contains("r1/3 c1/2"));
    assert!(at(vec![kc(KeyCode::Down)]).contains("r2/3"));
    assert!(at(vec![kc(KeyCode::Right)]).contains("c2/2"));
    assert!(at(vec![kc(KeyCode::Right), kc(KeyCode::Left)]).contains("c1/2"));
    assert!(at(vec![kc(KeyCode::Down), kc(KeyCode::Up)]).contains("r1/3"));
    assert!(at(vec![kc(KeyCode::End)]).contains("r3/3"));
    assert!(at(vec![kc(KeyCode::End), kc(KeyCode::Home)]).contains("r1/3"));
}

#[test]
fn the_page_keys_move_by_a_screenful_and_clamp() {
    let (_d, p) = file("l.grid", &long(100));
    let t = |ev: Vec<AppEvent>| title(&run(&p, SIZE, ev));
    let down = t(vec![ctrl('f')]);
    assert!(!down.contains("r1/100"), "{down}");
    // 12 rows - 2 borders - the footer - the header and its rule = a 7-row screenful.
    assert!(down.contains("r8/100"), "{down}");
    let two = t(vec![ctrl('f'), ctrl('b')]);
    assert!(two.contains("r1/100"), "{two}");
    let pd = t(vec![kc(KeyCode::PageDown)]);
    assert_eq!(
        pd.replace(' ', ""),
        down.replace(' ', ""),
        "PageDown = Ctrl-f"
    );
    let half = t(vec![ctrl('d')]);
    assert!(half.contains("r4/100"), "half a screenful: {half}");
    assert!(t(vec![ctrl('d'), ctrl('u')]).contains("r1/100"));
    assert!(
        t(vec![kc(KeyCode::PageUp)]).contains("r1/100"),
        "clamps at the top"
    );
    let end = t((0..40).map(|_| ctrl('f')).collect());
    assert!(end.contains("r100/100"), "{end}");
}

#[test]
fn the_view_scrolls_with_the_cursor() {
    let (_d, p) = file("l.grid", &long(100));
    let rows = run(&p, SIZE, chars("G"));
    let s = all(&rows);
    assert!(s.contains("r099"), "{s}");
    assert!(!s.contains("r000"), "{s}");
    let rows = run(&p, SIZE, chars("Gg"));
    assert!(all(&rows).contains("r000"));
}

#[test]
fn an_empty_table_keeps_its_header_and_ignores_the_cursor_keys() {
    let (_d, p) = file("e.grid", "name,age\n");
    let rows = run(&p, SIZE, chars("jjGglh$0"));
    let s = all(&rows);
    assert!(title(&rows).contains("0×2"), "{s}");
    assert!(rows[1].contains("name"), "{s}");
    assert!(title(&rows).contains("r1/1 c1/2"), "{s}");
    // The keys moved nothing, and the app is still running.
    let (_, _, left) = run_counted(&p, SIZE, chars("jjGglh$0"));
    assert_eq!(left, 0);
}

#[test]
fn a_table_with_no_cells_at_all_is_drawn_and_has_no_cell_hint() {
    let (_d, p) = file("z.grid", "");
    let (rows, _, left) = run_counted(&p, SIZE, chars("jkhlGg"));
    assert_eq!(left, 0);
    assert!(!footer(&rows).contains("view cell"), "{}", all(&rows));
    let (rows, _, _) = run_counted(&p, SIZE, vec![kc(KeyCode::Enter)]);
    assert!(!all(&rows).contains("Cell ·"), "{}", all(&rows));
}

#[test]
fn a_one_cell_table_has_one_place_to_stand() {
    let (_d, p) = file("o.grid", "h\nx\n");
    let rows = run(&p, SIZE, chars("jlGg$0jlkh"));
    assert!(title(&rows).contains("r1/1 c1/1"), "{}", all(&rows));
}

#[test]
fn full_width_cells_are_drawn_and_navigated_by_cell() {
    let (_d, p) = file("w.grid", "名前,年齢\n山田太郎,三十\n鈴木,四十一\n");
    let rows = run(&p, SIZE, chars("jl"));
    let s = all(&rows);
    assert!(s.contains("山田太郎") && s.contains("四十一"), "{s}");
    assert!(title(&rows).contains("r2/2 c2/2"), "{s}");
    // The border stays a rectangle: the frame rows span the whole width, and each body row
    // ends with the right border in the last column.
    let w = |l: &String| unicode_width::UnicodeWidthStr::width(l.as_str());
    assert_eq!(w(&rows[0]), 80, "{s}");
    assert_eq!(w(&rows[10]), 80, "{s}");
    assert!(
        rows[1..10].iter().all(|l| w(l) == 80 && l.ends_with('│')),
        "{s}"
    );
}

#[test]
fn a_cell_key_does_not_ask_the_plugin_for_a_new_view() {
    let (_d, p) = book();
    let (_, views, _) = run_counted(&p, SIZE, chars("jlhkGg$0"));
    assert_eq!(views, 1, "moving the cursor reads nothing");
    let (_, views, _) = run_counted(&p, SIZE, vec![kc(KeyCode::Enter), kc(KeyCode::Esc)]);
    assert_eq!(views, 1, "the popup reads nothing");
}

#[test]
fn j_and_k_turn_the_sheets_and_clamp_at_the_ends() {
    let (_d, p) = book();
    let t = |ev: Vec<AppEvent>| run(&p, SIZE, ev);
    let r = t(chars("J"));
    assert!(title(&r).contains("Sheet2 (2/3)"), "{}", all(&r));
    assert!(all(&r).contains("│1 e"), "{}", all(&r));
    let r = t(chars("JJ"));
    assert!(title(&r).contains("Sheet3 (3/3)"), "{}", all(&r));
    assert!(all(&r).contains("│2 i"), "{}", all(&r));
    let r = t(chars("JJJJ"));
    assert!(
        title(&r).contains("Sheet3 (3/3)"),
        "stops at the last sheet"
    );
    let r = t(chars("K"));
    assert!(
        title(&r).contains("Sheet1 (1/3)"),
        "stops at the first sheet"
    );
    let r = t(chars("JK"));
    assert!(title(&r).contains("Sheet1 (1/3)"));
    let (_, views, _) = run_counted(&p, SIZE, chars("J"));
    assert_eq!(views, 2, "a turn asks the plugin for the sheet");
    let (_, views, _) = run_counted(&p, SIZE, chars("JJJJJ"));
    assert_eq!(views, 3, "a turn past the end asks for nothing");
    let (_, views, _) = run_counted(&p, SIZE, chars("KKK"));
    assert_eq!(views, 1, "a turn before the start asks for nothing");
}

#[test]
fn a_sheet_turn_puts_the_cursor_back_and_closes_the_popup() {
    let (_d, p) = book();
    let r = run(&p, SIZE, chars("jlJ"));
    assert!(title(&r).contains("A1"), "{}", all(&r));
    assert!(title(&r).contains("Sheet2"), "{}", all(&r));
    let mut ev = chars("jl");
    ev.push(kc(KeyCode::Enter));
    ev.push(kk('J'));
    // 'J' while the popup is open does not turn the sheet (a popup key set only).
    let r = run(&p, SIZE, ev);
    assert!(title(&r).contains("Sheet1"), "{}", all(&r));
    assert!(all(&r).contains("Cell ·"), "{}", all(&r));
    let mut ev = vec![kc(KeyCode::Enter), kc(KeyCode::Esc), kk('J')];
    let r = run(&p, SIZE, std::mem::take(&mut ev));
    assert!(title(&r).contains("Sheet2"), "{}", all(&r));
    assert!(!all(&r).contains("Cell ·"), "{}", all(&r));
}

#[test]
fn a_plain_csv_has_no_pages_and_j_k_capitals_do_nothing() {
    let (_d, p) = people();
    let a = run(&p, SIZE, vec![]);
    let b = run(&p, SIZE, chars("JKJK"));
    assert_eq!(a, b);
}

#[test]
fn enter_opens_the_full_cell_popup_and_esc_closes_it() {
    let (_d, p) = people();
    let r = run(&p, SIZE, vec![kc(KeyCode::Enter)]);
    let s = all(&r);
    assert!(s.contains("Cell · name"), "{s}");
    assert!(s.contains("ann"), "{s}");
    let r = run(&p, SIZE, vec![kc(KeyCode::Enter), kc(KeyCode::Esc)]);
    assert!(!all(&r).contains("Cell ·"), "{}", all(&r));
    let (r, _, left) = run_counted(&p, SIZE, vec![kc(KeyCode::Enter), kc(KeyCode::Esc)]);
    assert_eq!(left, 0);
    assert!(!all(&r).contains("Cell ·"));
    let r = run(&p, SIZE, vec![kc(KeyCode::Enter), kc(KeyCode::Enter)]);
    assert!(!all(&r).contains("Cell ·"), "Enter toggles");
}

#[test]
fn q_closes_the_popup_first_and_then_quits() {
    let (_d, p) = people();
    let (r, _, left) = run_counted(&p, SIZE, vec![kc(KeyCode::Enter), kk('q'), kk('j')]);
    assert_eq!(left, 0, "q did not quit while the popup was open");
    assert!(!all(&r).contains("Cell ·"));
    assert!(
        title(&r).contains("r2/3"),
        "the key after the close moved the cursor"
    );
    let (_, _, left) = run_counted(&p, SIZE, vec![kk('q'), kk('j')]);
    assert_eq!(left, 1, "q quits the table");
}

#[test]
fn the_popup_names_the_cell_it_shows_and_follows_the_cursor() {
    let (_d, p) = people();
    let mut ev = chars("jl");
    ev.push(kc(KeyCode::Enter));
    let r = run(&p, SIZE, ev);
    let s = all(&r);
    assert!(s.contains("Cell · age"), "{s}");
    assert!(s.contains("41"), "{s}");
    let (_d, b) = book();
    let mut ev = chars("jl");
    ev.push(kc(KeyCode::Enter));
    let r = run(&b, SIZE, ev);
    let s = all(&r);
    assert!(s.contains("Cell · B2"), "{s}");
    assert!(
        s.contains("value: d"),
        "the plugin's detail lines are shown: {s}"
    );
}

#[test]
fn the_cursor_stays_while_the_popup_is_open() {
    let (_d, p) = people();
    for keys in ["l", "h", "$", "0", "J", "K"] {
        let mut ev = vec![kc(KeyCode::Enter)];
        ev.extend(chars(keys));
        let r = run(&p, SIZE, ev);
        assert!(all(&r).contains("Cell · name"), "{keys}: {}", all(&r));
        assert!(title(&r).contains("r1/3 c1/2"), "{keys}: {}", all(&r));
    }
}

/// The text inside the popup frame only (the table behind it shows the first lines too).
fn popup_body(rows: &[String]) -> String {
    let top = rows
        .iter()
        .position(|l| l.contains("┌ Cell"))
        .expect("a popup is open");
    let col = rows[top][..rows[top].find("┌ Cell").unwrap()]
        .chars()
        .count();
    let mut out = Vec::new();
    for l in &rows[top + 1..] {
        if l.chars().nth(col) == Some('└') {
            break;
        }
        let inner: String = l.chars().skip(col + 1).collect();
        out.push(inner.split('│').next().unwrap_or("").trim_end().to_string());
    }
    out.join("\n")
}

fn tall_cell() -> (TmpDir, PathBuf) {
    let body: Vec<String> = (0..40).map(|i| format!("L{i:02}")).collect();
    file("t.grid", &format!("h\n{}\n", body.join("\\n")))
}

#[test]
fn the_popup_scrolls_with_the_motion_keys() {
    let (_d, p) = tall_cell();
    let size = (60, 16);
    let open = |keys: Vec<AppEvent>| {
        let mut ev = vec![kc(KeyCode::Enter)];
        ev.extend(keys);
        popup_body(&run(&p, size, ev))
    };
    let top = open(vec![]);
    assert!(top.contains("L00") && !top.contains("L39"), "{top}");
    let down = open(chars("jj"));
    assert!(!down.contains("L00") && down.contains("L02"), "{down}");
    assert!(open(chars("jjk")).contains("L01"));
    assert!(open(chars("jjkkkk")).contains("L00"), "clamps at the top");
    let bottom = open(chars("G"));
    assert!(
        bottom.contains("L39") && !bottom.contains("L00"),
        "{bottom}"
    );
    assert!(open(chars("Gg")).contains("L00"));
    let paged = open(vec![kc(KeyCode::PageDown)]);
    assert!(!paged.contains("L00"), "{paged}");
    let back = open(vec![kc(KeyCode::PageDown), kc(KeyCode::PageUp)]);
    assert!(back.contains("L00"), "{back}");
    let arrows = open(vec![kc(KeyCode::Down)]);
    assert!(
        arrows.contains("L01") && !arrows.contains("L00"),
        "{arrows}"
    );
    // A key that is not a popup key moves nothing.
    assert_eq!(open(chars("l")), top);
    assert_eq!(open(chars("$")), top);
}

#[test]
fn a_reopened_popup_starts_at_the_top() {
    let (_d, p) = tall_cell();
    let mut ev = vec![kc(KeyCode::Enter)];
    ev.extend(chars("G"));
    ev.push(kc(KeyCode::Esc));
    ev.push(kc(KeyCode::Enter));
    let s = all(&run(&p, (60, 16), ev));
    assert!(s.contains("L00"), "{s}");
}

#[test]
fn the_popup_follows_a_resize_by_redrawing_in_the_new_size() {
    let (_d, p) = tall_cell();
    let ev = vec![kc(KeyCode::Enter)];
    let small = all(&run(&p, (40, 10), ev.clone()));
    let big = all(&run(&p, (100, 30), ev));
    assert!(small.contains("L00") && big.contains("L00"));
    assert!(
        big.contains("L20"),
        "a taller screen shows more lines: {big}"
    );
    assert!(!small.contains("L20"), "{small}");
}

#[test]
fn the_help_lists_the_table_keys_and_closes_on_escape() {
    let (_d, p) = people();
    let r = run(&p, (80, 24), vec![kk('?')]);
    let s = all(&r);
    assert!(s.contains("CSV / TSV / spreadsheet"), "{s}");
    assert!(s.contains("move between cells"), "{s}");
    assert!(s.contains("view the full cell text"), "{s}");
    assert!(s.contains("Help"), "{s}");
    let r = run(&p, (80, 24), vec![kk('?'), kc(KeyCode::Esc)]);
    assert!(!all(&r).contains("Help  (?"), "{}", all(&r));
    assert!(all(&r).contains("name"), "the table is back");
}

#[test]
fn the_help_of_a_paged_table_names_the_page_keys() {
    let (_d, p) = book();
    let s = all(&run(&p, (80, 30), vec![kk('?')]));
    assert!(s.contains("J / K"), "{s}");
    // The main app lists J/K only when the preview has pages to turn (`can_turn_pages`).
    let (_d, q) = people();
    let s = all(&run(&q, (80, 30), vec![kk('?')]));
    assert!(!s.contains("J / K"), "a single table has no pages: {s}");
}

#[test]
fn the_footer_shows_a_hint_only_for_a_key_that_acts() {
    let (_d, p) = people();
    let f = footer(&run(&p, SIZE, vec![]));
    assert!(f.contains("h/j/k/l:cell"), "{f}");
    assert!(f.contains("view cell"), "{f}");
    assert!(!f.contains("J/K"), "no pages in a csv: {f}");
    assert!(
        f.contains("g/G:ends") && f.contains("q:quit") && f.contains("?:help"),
        "{f}"
    );
    let (_d, b) = book();
    let f = footer(&run(&b, SIZE, vec![]));
    assert!(f.contains("J/K:sheet"), "{f}");
    let f = footer(&run(&b, SIZE, vec![kc(KeyCode::Enter)]));
    assert!(f.contains("j/k:scroll"), "{f}");
    assert!(f.contains("g/G:top/bottom"), "{f}");
    assert!(f.contains("Esc:close"), "{f}");
    assert!(!f.contains("J/K"), "the popup does not turn pages: {f}");
    assert!(!f.contains("h/j/k/l"), "{f}");
}

#[test]
fn a_single_sheet_book_shows_no_page_hint() {
    let (_d, p) = file("s.book", "a,b\n");
    let f = footer(&run(&p, SIZE, vec![]));
    assert!(!f.contains("J/K"), "{f}");
    let a = run(&p, SIZE, vec![]);
    assert_eq!(a, run(&p, SIZE, chars("JK")));
}

#[test]
fn views_are_counted_per_open_not_shared_between_runs() {
    let (_d, p) = book();
    let (_, a, _) = run_counted(&p, SIZE, vec![]);
    let (_, b, _) = run_counted(&p, SIZE, vec![]);
    assert_eq!((a, b), (1, 1));
}

// ---------------------------------------------------------------------------------------------
// Property: every key the table's footer and help name acts, and `table_acts_on` is the loop.
// ---------------------------------------------------------------------------------------------

use konoma_host_core::lang::Lang;
use konoma_host_core::shared::SharedConfig;
use konoma_plugin::app::TableMode;

use crate::help::{row_keys, token_keys};

fn keys_of(config: &str, lang: Lang) -> AppKeys {
    let c = SharedConfig::parse(config).expect("the test config parses");
    AppKeys::new(&c.keys, &c.ui.keys).with_lang(lang)
}

fn table_configs() -> Vec<(&'static str, &'static str)> {
    vec![
        ("default", ""),
        ("unbind j", "[keys.table]\nj = \"noop\"\n"),
        (
            "unbind j k h l",
            "[keys.table]\nj = \"noop\"\nk = \"noop\"\nh = \"noop\"\nl = \"noop\"\n",
        ),
        ("unbind q", "[keys.table]\nq = \"noop\"\n"),
        (
            "unbind q Q",
            "[keys.table]\nq = \"noop\"\n[keys.global]\nQ = \"noop\"\n",
        ),
        (
            "rebind page turn",
            "[keys.table]\nn = \"page_next\"\nJ = \"noop\"\n",
        ),
        (
            "page keys rebound",
            "[keys.table]\n\"m\" = \"navigate:page_down\"\n",
        ),
        ("less", "[ui]\nkeys = \"less\"\n"),
        ("full width", "[keys.table]\n\"あ\" = \"navigate:down\"\n"),
    ]
}

fn all_modes() -> Vec<TableMode> {
    let mut v = Vec::new();
    for popup in [false, true] {
        for paged in [false, true] {
            for cells in [false, true] {
                // A popup shows a cell: it is never open over a table that has none.
                if popup && !cells {
                    continue;
                }
                v.push(TableMode {
                    popup,
                    paged,
                    cells,
                    sheets: false,
                });
            }
        }
    }
    v
}

/// The keys of a table help row (`h / j / k / l / arrows` names the four arrow keys).
fn help_row_keys(text: &str) -> Vec<Key> {
    row_keys(&text.replace("arrows", "\u{2191} \u{2193} \u{2190} \u{2192}"))
}

#[test]
fn every_key_the_table_footer_and_help_name_acts() {
    let (mut tokens, mut rows) = (0, 0);
    for (name, cfg) in table_configs() {
        for lang in [Lang::En, Lang::Jp] {
            let k = keys_of(cfg, lang);
            for mode in all_modes() {
                let hints = k.table_footer_hints(mode);
                assert!(!hints.is_empty(), "{name}: a footer always has the way out");
                for t in &hints {
                    let parsed = token_keys(&t.text);
                    assert_eq!(parsed.len(), t.keys.len(), "{name} {}: {t:?}", t.text);
                    for key in parsed {
                        assert!(
                            k.table_acts_on(&key, false, mode),
                            "{name} {lang:?} {mode:?}: the footer names {key:?} in `{}` but it does nothing",
                            t.text
                        );
                        tokens += 1;
                    }
                }
                for sec in k.table_help_sections(mode) {
                    for (keys_text, desc) in &sec.rows {
                        for key in help_row_keys(keys_text) {
                            // The help is open: a motion scrolls it, and the keys that need a
                            // cell or a page turn are named only when they act.
                            // The help describes the table under a popup: its rows are those of
                            // the table with no popup.
                            let under = TableMode {
                                popup: false,
                                ..mode
                            };
                            assert!(
                                k.table_acts_on(&key, true, under)
                                    || k.table_acts_on(&key, false, under),
                                "{name} {lang:?} {mode:?}: the help names {key:?} for `{desc}` but it does nothing"
                            );
                            rows += 1;
                        }
                    }
                }
            }
        }
    }
    assert!(tokens > 500, "{tokens}");
    assert!(rows > 800, "{rows}");
}

#[test]
fn the_footer_and_help_leave_out_what_does_not_act() {
    let k = keys_of("", Lang::En);
    let none = TableMode::default();
    let has =
        |mode: TableMode, text: &str| k.table_footer_tokens(mode).iter().any(|t| t.contains(text));
    assert!(!has(none, "J/K"), "no pages");
    assert!(has(
        TableMode {
            paged: true,
            ..none
        },
        "J/K"
    ));
    assert!(!has(none, "view cell"), "no cell");
    assert!(has(
        TableMode {
            cells: true,
            ..none
        },
        "view cell"
    ));
    assert!(!has(
        TableMode {
            popup: true,
            paged: true,
            cells: true,
            sheets: false
        },
        "J/K"
    ));
    assert!(!has(
        TableMode {
            popup: true,
            paged: true,
            cells: true,
            sheets: false
        },
        "h/j/k/l"
    ));
    let help = |mode: TableMode| {
        k.table_help_sections(mode)
            .iter()
            .flat_map(|s| s.rows.clone())
            .map(|(a, b)| format!("{a} {b}"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    assert!(!help(none).contains("J / K"));
    assert!(help(TableMode {
        paged: true,
        ..none
    })
    .contains("J / K"));
    assert!(!help(none).contains("view the full cell"));
    assert!(help(TableMode {
        cells: true,
        ..none
    })
    .contains("view the full cell"));
}

/// 6 rows x 3 columns, with a sheet before and after when `book`.
fn middle_table(book: bool) -> (TmpDir, PathBuf) {
    let sheet = "a,b,c\n1,2,3\n4,5,6\n7,8,9\n10,11,12\n13,14,15\n16,17,18\n";
    if book {
        file("m.book", &format!("{sheet}===\n{sheet}===\n{sheet}"))
    } else {
        file("m.grid", sheet)
    }
}

const REAL: (u16, u16) = (100, 8);

fn key_candidates() -> Vec<Key> {
    let mut v: Vec<Key> = "hjklgG0$JKeoqQ?xnm \u{3042}Hz"
        .chars()
        .map(Key::char)
        .collect();
    for c in ['f', 'b', 'd', 'u', 'c'] {
        v.push(Key::ctrl(c));
    }
    for code in [
        KeyCode::Down,
        KeyCode::Up,
        KeyCode::Left,
        KeyCode::Right,
        KeyCode::Home,
        KeyCode::End,
        KeyCode::Esc,
        KeyCode::Enter,
        KeyCode::Tab,
        KeyCode::PageDown,
        KeyCode::PageUp,
    ] {
        v.push(Key::new(code));
    }
    v
}

/// Whether `key` changes the screen (or ends the loop) after `setup`.
fn acts_for_real(k: &AppKeys, p: &Path, setup: &[AppEvent], key: &Key) -> bool {
    let observe = |pressed: Option<&Key>| {
        let mut ev = setup.to_vec();
        if let Some(x) = pressed {
            ev.push(AppEvent::Key(x.clone()));
        }
        ev.push(AppEvent::Resize);
        let (rows, _, left) = run_keyed(p, REAL, ev, k);
        assert!(left <= 1, "{left} events left");
        (rows, left == 1)
    };
    let (before, bq) = observe(None);
    assert!(!bq, "the set-up alone ends the loop");
    let (after, quit) = observe(Some(key));
    quit || after != before
}

#[test]
fn what_acts_over_a_table_is_what_the_loop_does() {
    let (mut checked, mut yes, mut no, mut in_help, mut in_popup) = (0, 0, 0, 0, 0);
    let (_d1, grid_p) = middle_table(false);
    let (_d2, book_p) = middle_table(true);
    let (_d3, tall_p) = tall_cell();
    for (name, cfg) in table_configs() {
        let k = keys_of(cfg, Lang::En);
        let opener = k
            .table_footer_hints(TableMode::default())
            .pop()
            .and_then(|t| token_keys(&t.text).into_iter().next());
        // (file, mode, set-up): the cursor is moved off the edges first so a motion that acts
        // changes the screen whichever way it goes.
        let mid: Vec<AppEvent> = vec![kc(KeyCode::Down), kc(KeyCode::Down), kc(KeyCode::Right)];
        // On the middle sheet both `J` and `K` turn (at an end the other is clamped).
        // (`J` is the key unless the config moved the page turn to another one.)
        let paged = TableMode {
            cells: true,
            paged: true,
            ..Default::default()
        };
        let next_sheet = ['J', 'n']
            .into_iter()
            .find(|c| k.table_acts_on(&Key::new(KeyCode::Char(*c)), false, paged))
            .map_or_else(|| kk('J'), kk);
        let mut on_sheet2 = vec![next_sheet.clone()];
        on_sheet2.extend(mid.clone());
        let mut cases: Vec<(&Path, TableMode, Vec<AppEvent>, bool, bool)> = vec![
            (
                &grid_p,
                TableMode {
                    cells: true,
                    ..Default::default()
                },
                mid.clone(),
                false,
                false,
            ),
            (
                &book_p,
                TableMode {
                    cells: true,
                    paged: true,
                    ..Default::default()
                },
                on_sheet2,
                false,
                false,
            ),
        ];
        let mut popup_setup = vec![kc(KeyCode::Enter)];
        popup_setup.extend((0..3).map(|_| kc(KeyCode::Down)));
        cases.push((
            &tall_p,
            TableMode {
                cells: true,
                popup: true,
                ..Default::default()
            },
            popup_setup,
            false,
            true,
        ));
        if let Some(o) = &opener {
            let mut s = vec![AppEvent::Key(o.clone())];
            s.extend((0..3).map(|_| kc(KeyCode::Down)));
            cases.push((
                &grid_p,
                TableMode {
                    cells: true,
                    ..Default::default()
                },
                s.clone(),
                true,
                false,
            ));
            let mut sb = vec![next_sheet.clone()];
            sb.extend(s);
            cases.push((
                &book_p,
                TableMode {
                    cells: true,
                    paged: true,
                    ..Default::default()
                },
                sb,
                true,
                false,
            ));
        }
        for (path, mode, setup, helping, popup) in cases {
            for key in key_candidates() {
                let oracle = k.table_acts_on(&key, helping, mode);
                let real = acts_for_real(&k, path, &setup, &key);
                assert_eq!(
                    oracle, real,
                    "{name} {mode:?} help={helping}: table_acts_on({key:?}) = {oracle} but the loop {}",
                    if real { "acts" } else { "does nothing" }
                );
                checked += 1;
                yes += usize::from(real);
                no += usize::from(!real);
                in_help += usize::from(helping);
                in_popup += usize::from(popup);
            }
        }
    }
    assert!(checked > 1500, "{checked}");
    assert!(yes > 300 && no > 300, "{yes} {no}");
    assert!(in_help > 300 && in_popup > 300, "{in_help} {in_popup}");
}

/// `Enter` over a table with no cell does nothing, so the key is not said to act (the hint and
/// the help row are left out for it); over a table with cells it acts.
#[test]
fn enter_acts_only_over_a_table_with_a_cell() {
    let k = keys_of("", Lang::En);
    let enter = Key::new(KeyCode::Enter);
    let none = TableMode::default();
    let cells = TableMode {
        cells: true,
        ..none
    };
    assert!(!k.table_acts_on(&enter, false, none));
    assert!(k.table_acts_on(&enter, false, cells));
    // Said to act = acts for real.
    let (_d1, empty) = file("z.grid", "");
    let (_d2, full) = middle_table(false);
    assert!(!acts_for_real(&k, &empty, &[], &enter));
    assert!(acts_for_real(&k, &full, &[], &enter));
}

/// A book's page keys are called `sheet`; a paged table without sheets calls them `page`.
#[test]
fn the_page_keys_are_called_sheet_only_for_sheets() {
    let k = keys_of("", Lang::En);
    let base = TableMode {
        paged: true,
        cells: true,
        ..TableMode::default()
    };
    let text = |m: TableMode| k.table_footer_tokens(m).join("  ");
    let help = |m: TableMode| {
        k.table_help_sections(m)
            .iter()
            .flat_map(|s| s.rows.clone())
            .map(|(a, b)| format!("{a} {b}"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let sheets = TableMode {
        sheets: true,
        ..base
    };
    assert!(text(sheets).contains("J/K:sheet"), "{}", text(sheets));
    assert!(
        help(sheets).contains("next / previous sheet"),
        "{}",
        help(sheets)
    );
    assert!(!text(base).contains("sheet"), "{}", text(base));
    assert!(text(base).contains("J/K:page"), "{}", text(base));
    assert!(!help(base).contains("sheet"), "{}", help(base));
}
