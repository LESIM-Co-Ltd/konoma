//! The loop on a `TestBackend`: what is drawn, what the keys do, when it ends.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use konoma_plugin::app::{open_file, run_with, AppCx, AppEvent, AppKeys, EventSource, Next};
use konoma_plugin::prelude::*;
use konoma_plugin::TermCaps;
use konoma_test_support::unique_tmp;
use ratatui::backend::TestBackend;
use ratatui::style::Color as RColor;
use ratatui::Terminal;

use crate::plugin::TestPlugin;

fn cx() -> AppCx {
    AppCx {
        lang: "en".into(),
        term: TermCaps::new(),
        title: "title".into(),
        ..AppCx::default()
    }
}

fn key(c: char) -> AppEvent {
    AppEvent::Key(Key::char(c))
}

fn code(k: KeyCode) -> AppEvent {
    AppEvent::Key(Key::new(k))
}

/// Write `name` with `body` into a fresh directory; the guard keeps it alive.
fn file(tag: &str, name: &str, body: &str) -> (konoma_test_support::TmpDir, std::path::PathBuf) {
    let dir = unique_tmp(tag);
    std::fs::create_dir_all(dir.as_path()).unwrap();
    let p = dir.as_path().join(name);
    std::fs::write(&p, body).unwrap();
    (dir, p)
}

fn lines20() -> String {
    (0..20).map(|i| format!("line {i:02}\n")).collect()
}

/// Run the app over `path` with `events` on a `w` x `h` screen; returns the terminal (the last
/// drawn frame) and how many events were left unread.
fn run_events(
    plugin: TestPlugin,
    path: &Path,
    (w, h): (u16, u16),
    events: Vec<AppEvent>,
) -> (Terminal<TestBackend>, usize) {
    let mut opened = open_file(vec![Arc::new(plugin)], path, "en", TermCaps::new()).expect("opens");
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    let mut it = events.into_iter();
    // What `run` hands the loop: the file and what the plugin declared it shows when it cannot.
    let cx = AppCx {
        path: opened.path.clone(),
        unavailable: opened.unavailable.clone(),
        ..cx()
    };
    run_with(
        &mut term,
        &mut *opened.session,
        &mut it,
        &cx,
        &AppKeys::defaults(),
    )
    .expect("runs");
    (term, it.count())
}

fn rows(term: &Terminal<TestBackend>) -> Vec<String> {
    let buf = term.backend().buffer();
    (0..buf.area.height)
        .map(|y| {
            (0..buf.area.width)
                .map(|x| buf[(x, y)].symbol().to_string())
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect()
}

fn first_body(term: &Terminal<TestBackend>) -> String {
    rows(term)[0].clone()
}

#[test]
fn a_text_file_is_drawn_from_its_first_line_with_a_footer_on_the_last_row() {
    let (_d, p) = file("app_text", "a.t", &lines20());
    let (term, _) = run_events(TestPlugin::all(), &p, (60, 6), vec![]);
    let r = rows(&term);
    assert_eq!(r.len(), 6);
    for (i, row) in r.iter().enumerate().take(5) {
        assert_eq!(row, &format!("line {i:02}"), "row {i}");
    }
    assert!(r[5].contains("line 1"), "{r:?}");
    assert!(r[5].ends_with("title"), "{r:?}");
    assert!(r[5].contains("q:quit"), "{r:?}");
    assert!(!r[4].contains("q:quit"));
    // The footer row is reversed.
    let buf = term.backend().buffer();
    assert!(buf[(0, 5)]
        .modifier
        .contains(ratatui::style::Modifier::REVERSED));
    assert!(!buf[(0, 0)]
        .modifier
        .contains(ratatui::style::Modifier::REVERSED));
}

#[test]
fn j_and_k_and_the_arrows_scroll_by_one_line() {
    let (_d, p) = file("app_scroll", "a.t", &lines20());
    let at = |ev: Vec<AppEvent>| first_body(&run_events(TestPlugin::all(), &p, (24, 6), ev).0);
    assert_eq!(at(vec![key('j')]), "line 01");
    assert_eq!(at(vec![key('j'), key('j'), key('j')]), "line 03");
    assert_eq!(
        at(vec![code(KeyCode::Down), code(KeyCode::Down)]),
        "line 02"
    );
    assert_eq!(at(vec![key('j'), key('j'), key('j'), key('k')]), "line 02");
    assert_eq!(at(vec![key('j'), key('j'), code(KeyCode::Up)]), "line 01");
    // At the top, `k` stays at the top.
    assert_eq!(at(vec![key('k'), key('k')]), "line 00");
}

#[test]
fn scrolling_stops_where_the_last_page_is_full() {
    let (_d, p) = file("app_end", "a.t", &lines20());
    // 5 body rows, 20 lines: the first row of the last page is line 15.
    let many: Vec<AppEvent> = (0..40).map(|_| key('j')).collect();
    let (term, _) = run_events(TestPlugin::all(), &p, (24, 6), many);
    let r = rows(&term);
    assert_eq!(r[0], "line 15");
    assert_eq!(r[4], "line 19");
}

#[test]
fn page_keys_home_and_end_move_by_pages() {
    let (_d, p) = file("app_page", "a.t", &lines20());
    let at = |ev: Vec<AppEvent>| first_body(&run_events(TestPlugin::all(), &p, (24, 6), ev).0);
    assert_eq!(at(vec![code(KeyCode::PageDown)]), "line 05");
    assert_eq!(
        at(vec![code(KeyCode::PageDown), code(KeyCode::PageUp)]),
        "line 00"
    );
    assert_eq!(at(vec![code(KeyCode::End)]), "line 15");
    assert_eq!(at(vec![code(KeyCode::End), code(KeyCode::Home)]), "line 00");
    assert_eq!(at(vec![key('G'), key('g')]), "line 00");
}

#[test]
fn a_short_text_does_not_scroll_at_all() {
    let (_d, p) = file("app_short", "a.t", "one\ntwo\n");
    let (term, _) = run_events(TestPlugin::all(), &p, (24, 6), vec![key('j'), key('j')]);
    assert_eq!(rows(&term)[0], "one");
}

#[test]
fn q_ends_the_loop_and_later_keys_are_not_read() {
    let (_d, p) = file("app_q", "a.t", &lines20());
    let (term, left) = run_events(
        TestPlugin::all(),
        &p,
        (24, 6),
        vec![key('j'), key('q'), key('j'), key('j')],
    );
    assert_eq!(left, 2, "the keys after q stay unread");
    assert_eq!(first_body(&term), "line 01");
}

#[test]
fn ctrl_c_ends_the_loop_too() {
    let (_d, p) = file("app_ctrlc", "a.t", &lines20());
    let (_, left) = run_events(
        TestPlugin::all(),
        &p,
        (24, 6),
        vec![AppEvent::Key(Key::ctrl('c')), key('j')],
    );
    assert_eq!(left, 1);
}

#[test]
fn a_modified_q_does_not_end_the_loop_and_does_not_scroll() {
    let (_d, p) = file("app_modq", "a.t", &lines20());
    let alt_q = AppEvent::Key(Key::char('q').with_mods(KeyMods::ALT));
    let ctrl_j = AppEvent::Key(Key::ctrl('j'));
    let (term, left) = run_events(
        TestPlugin::all(),
        &p,
        (24, 6),
        vec![alt_q, ctrl_j, key('j')],
    );
    assert_eq!(left, 0);
    assert_eq!(first_body(&term), "line 01");
}

#[test]
fn the_loop_ends_when_the_events_end() {
    let (_d, p) = file("app_evend", "a.t", "x\n");
    let (_, left) = run_events(TestPlugin::all(), &p, (24, 6), vec![]);
    assert_eq!(left, 0);
}

#[test]
fn an_owned_string_scrolls_the_same_way() {
    let (_d, p) = file("app_owned", "a.o", &lines20());
    let (term, _) = run_events(TestPlugin::all(), &p, (24, 6), vec![key('j'), key('j')]);
    let r = rows(&term);
    assert_eq!(r[0], "line 02");
    assert_eq!(r[4], "line 06");
    let many: Vec<AppEvent> = (0..40).map(|_| key('j')).collect();
    let (term, _) = run_events(TestPlugin::all(), &p, (24, 6), many);
    assert_eq!(first_body(&term), "line 15");
}

#[test]
fn styled_lines_keep_their_style_and_scroll() {
    let (_d, p) = file("app_lines", "a.l", &lines20());
    let (term, _) = run_events(TestPlugin::all(), &p, (24, 6), vec![]);
    let buf = term.backend().buffer();
    assert_eq!(buf[(0, 0)].fg, RColor::Red);
    assert_eq!(buf[(0, 1)].fg, RColor::Reset);
    let (term, _) = run_events(TestPlugin::all(), &p, (24, 6), vec![key('j')]);
    assert_eq!(first_body(&term), "line 01");
    assert_eq!(term.backend().buffer()[(0, 0)].fg, RColor::Reset);
    let many: Vec<AppEvent> = (0..40).map(|_| key('j')).collect();
    let (term, _) = run_events(TestPlugin::all(), &p, (24, 6), many);
    assert_eq!(first_body(&term), "line 15");
}

#[test]
fn a_canvas_is_sized_to_the_screen_and_drawn_cell_for_cell() {
    let (_d, p) = file("app_canvas", "a.c", "");
    let (term, _) = run_events(TestPlugin::all(), &p, (10, 4), vec![]);
    let r = rows(&term);
    // 3 body rows of 10 cells; the first cell is the blue bold A.
    assert_eq!(r[0], "Axxxxxxxxx");
    assert_eq!(r[1], "xxxxxxxxxx");
    assert_eq!(r[2], "xxxxxxxxxx");
    assert!(
        r[3].contains("q:quit"),
        "the footer owns the last row: {r:?}"
    );
    let buf = term.backend().buffer();
    assert_eq!(buf[(0, 0)].fg, RColor::Blue);
    assert!(buf[(0, 0)]
        .modifier
        .contains(ratatui::style::Modifier::BOLD));
    assert_eq!(buf[(1, 0)].fg, RColor::Reset);
}

#[test]
fn a_canvas_bigger_than_the_screen_is_clipped_without_a_panic() {
    let (_d, p) = file("app_big", "a.big", "");
    let (term, _) = run_events(TestPlugin::all(), &p, (8, 3), vec![]);
    let r = rows(&term);
    assert_eq!(r[0], "Axxxxxxx");
    assert_eq!(r[1], "xxxxxxxx");
    assert!(r[2].contains("q:quit"));
}

#[test]
fn a_canvas_follows_the_safety_rules_of_the_host_painter() {
    use konoma_plugin::ratatui_draw::paint_canvas;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;

    let (_d, p) = file("app_hostile", "a.hostile", "");
    // 6 columns and 2 body rows: the 8-column canvas is cut at column 6, so the wide character
    // at column 5 of the first row would need column 6 and is not drawn.
    let (term, _) = run_events(TestPlugin::all(), &p, (6, 3), vec![]);
    let got = term.backend().buffer();
    let mut want = Buffer::empty(Rect::new(0, 0, 6, 2));
    paint_canvas(
        &mut want,
        Rect::new(0, 0, 6, 2),
        &crate::plugin::hostile_canvas(),
    );
    for y in 0..2u16 {
        for x in 0..6u16 {
            assert_eq!(
                got[(x, y)].symbol(),
                want[(x, y)].symbol(),
                "symbol {x},{y}"
            );
            assert_eq!(got[(x, y)].style(), want[(x, y)].style(), "style {x},{y}");
        }
    }
    // And the rules themselves, spelled out.
    let sym = |x: u16, y: u16| got[(x, y)].symbol().to_string();
    assert_eq!(sym(0, 0), "a");
    assert_eq!(sym(1, 0), " ", "a control character is a space");
    assert_eq!(sym(2, 0), "日");
    assert_eq!(
        got[(3, 0)].bg,
        RColor::Reset,
        "a continuation cell is skipped"
    );
    assert_eq!(sym(4, 0), " ", "the kitty placeholder is a space");
    assert_eq!(got[(4, 0)].fg, RColor::Green, "...and keeps its style");
    assert_eq!(
        sym(5, 0),
        " ",
        "a wide character cut by the edge is not drawn"
    );
    assert_eq!(sym(0, 1), " ");
    assert_eq!(sym(1, 1), "b");
    assert_eq!(sym(2, 1), " ");
    assert_eq!(sym(3, 1), "日");
    assert_eq!(
        got[(4, 1)].bg,
        RColor::Reset,
        "a continuation cell is skipped"
    );
    assert!(
        rows(&term)[2].contains("q:qui"),
        "the footer owns the last row"
    );
}

#[test]
fn a_one_row_screen_has_only_the_footer() {
    let (_d, p) = file("app_one", "a.c", "");
    let (term, _) = run_events(TestPlugin::all(), &p, (10, 1), vec![]);
    assert!(rows(&term)[0].contains("q:quit"));
    // And a zero-height screen does not panic.
    let (_term, _) = run_events(TestPlugin::all(), &p, (10, 0), vec![]);
}

/// Records the waits the loop asked for, answers `Timeout` for the first `timeouts` calls and
/// then ends.
struct Waits {
    seen: Vec<Option<Duration>>,
    timeouts: usize,
}

impl EventSource for Waits {
    fn next(&mut self, wait: Option<Duration>) -> Next {
        self.seen.push(wait);
        if self.seen.len() <= self.timeouts {
            Next::Timeout
        } else {
            Next::End
        }
    }
}

fn waits_for(plugin: TestPlugin, name: &str, timeouts: usize) -> Vec<Option<Duration>> {
    let (_d, p) = file("app_waits", name, "x\n");
    let mut opened = open_file(vec![Arc::new(plugin)], &p, "en", TermCaps::new()).unwrap();
    let mut term = Terminal::new(TestBackend::new(10, 4)).unwrap();
    let mut w = Waits {
        seen: Vec::new(),
        timeouts,
    };
    run_with(
        &mut term,
        &mut *opened.session,
        &mut w,
        &cx(),
        &AppKeys::defaults(),
    )
    .unwrap();
    w.seen
}

#[test]
fn a_canvas_that_asks_to_be_drawn_again_makes_the_loop_wait_that_long_and_draw_again() {
    let mut plugin = TestPlugin::all();
    plugin.redraw_ms = 2000;
    let seen = waits_for(plugin, "a.c", 2);
    assert_eq!(seen, vec![Some(Duration::from_secs(2)); 3]);
}

#[test]
fn a_redraw_wait_is_raised_to_33_ms_and_a_text_waits_for_ever() {
    let mut plugin = TestPlugin::all();
    plugin.redraw_ms = 1;
    assert_eq!(
        waits_for(plugin, "a.c", 0),
        vec![Some(Duration::from_millis(33))]
    );
    assert_eq!(waits_for(TestPlugin::all(), "a.t", 1), vec![None, None]);
}

#[test]
fn a_timeout_draws_again() {
    // The canvas is asked for its view again on a timeout: count the calls through the screen.
    struct Counting(std::sync::Arc<std::sync::atomic::AtomicUsize>);
    impl Session for Counting {
        fn view(&mut self, req: &ViewReq, _cx: &ViewCx) -> Result<View, Failure> {
            let n = self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let mut c = CanvasView::new(req.cols, req.rows);
            c.set_string(0, 0, &format!("n={n}"), Style::new());
            c.redraw_after = Some(Duration::from_millis(50));
            Ok(View::Canvas(c))
        }
    }
    let n = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut s = Counting(n.clone());
    let mut term = Terminal::new(TestBackend::new(10, 3)).unwrap();
    let mut w = Waits {
        seen: Vec::new(),
        timeouts: 3,
    };
    run_with(&mut term, &mut s, &mut w, &cx(), &AppKeys::defaults()).unwrap();
    assert_eq!(n.load(std::sync::atomic::Ordering::SeqCst), 4);
    assert_eq!(rows(&term)[0], "n=3");
}

#[test]
fn a_resize_event_asks_the_session_again() {
    struct Sizes(std::sync::Arc<std::sync::Mutex<Vec<(u16, u16)>>>);
    impl Session for Sizes {
        fn view(&mut self, req: &ViewReq, _cx: &ViewCx) -> Result<View, Failure> {
            self.0.lock().unwrap().push((req.cols, req.rows));
            Ok(View::Canvas(CanvasView::new(req.cols, req.rows)))
        }
    }
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut s = Sizes(seen.clone());
    let mut term = Terminal::new(TestBackend::new(10, 4)).unwrap();
    let mut ev = vec![AppEvent::Resize].into_iter();
    run_with(&mut term, &mut s, &mut ev, &cx(), &AppKeys::defaults()).unwrap();
    assert_eq!(*seen.lock().unwrap(), vec![(10, 3), (10, 3)]);
}

#[test]
fn a_message_view_is_drawn() {
    let (_d, p) = file("app_msg", "a.m", "");
    let (term, _) = run_events(TestPlugin::all(), &p, (30, 4), vec![]);
    assert_eq!(rows(&term)[0], "hello from app-test");
}

#[test]
fn a_view_that_is_not_drawn_yet_says_so_in_one_line() {
    let (_d, p) = file("app_list", "a.list", "");
    let (term, _) = run_events(TestPlugin::all(), &p, (40, 4), vec![]);
    assert_eq!(rows(&term)[0], "[not displayable yet: list]");
}

#[test]
fn a_failing_view_is_shown_not_fatal_and_q_still_quits() {
    let (_d, p) = file("app_fail", "a.fail", "");
    let (term, left) = run_events(TestPlugin::all(), &p, (300, 4), vec![key('q'), key('j')]);
    // The lines konoma itself shows for the same failure: the message, then the path.
    let r = rows(&term);
    assert_eq!(r[0].trim_end(), "broken on purpose");
    assert_eq!(r[1].trim_end(), p.display().to_string());
    assert_eq!(left, 1);
}

#[test]
fn a_long_line_and_a_tab_and_crlf_are_drawn_clean() {
    let body = format!("a\tb\r\n{}\nlast", "w".repeat(100_000));
    let (_d, p) = file("app_long", "a.t", &body);
    // Not wrapping: the long line is cut at the edge and `last` follows it.
    let (term, _) = run_shown(&p, (12, 5), vec![], no_wrap());
    let r = rows(&term);
    assert_eq!(r[0], "a    b");
    assert_eq!(r[1], "wwwwwwwwwwww");
    assert_eq!(r[2], "last");
    // Wrapping (the default): the long line goes on over the rows below it.
    let (term, _) = run_events(TestPlugin::all(), &p, (12, 5), vec![]);
    let r = rows(&term);
    assert_eq!(r[0], "a    b");
    assert_eq!(r[1], "wwwwwwwwwwww");
    assert_eq!(r[2], "wwwwwwwwwwww");
    assert_eq!(r[3], "wwwwwwwwwwww");
}

fn no_wrap() -> konoma_plugin::app::Display {
    konoma_plugin::app::Display {
        wrap: false,
        ..Default::default()
    }
}

/// [`run_events`] with the `[ui]` display settings `display` (the keys follow its `wrap`).
fn run_shown(
    path: &Path,
    (w, h): (u16, u16),
    events: Vec<AppEvent>,
    display: konoma_plugin::app::Display,
) -> (Terminal<TestBackend>, usize) {
    let mut opened = open_file(
        vec![Arc::new(TestPlugin::all())],
        path,
        "en",
        TermCaps::new(),
    )
    .expect("opens");
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    let mut it = events.into_iter();
    let cx = AppCx {
        path: opened.path.clone(),
        unavailable: opened.unavailable.clone(),
        display: display.clone(),
        ..cx()
    };
    run_with(
        &mut term,
        &mut *opened.session,
        &mut it,
        &cx,
        &AppKeys::defaults().with_wrap(display.wrap),
    )
    .expect("runs");
    (term, it.count())
}

#[test]
fn a_text_that_does_not_wrap_scrolls_sideways_by_four_and_to_both_ends() {
    let (_d, p) = file("app_hscroll", "a.t", "abcdefghijklmnopqrstuvwxyz\nshort\n");
    let at = |ev: Vec<AppEvent>| rows(&run_shown(&p, (10, 4), ev, no_wrap()).0)[0].clone();
    assert_eq!(at(vec![]), "abcdefghij");
    assert_eq!(at(vec![key('l')]), "efghijklmn");
    assert_eq!(at(vec![key('l'), key('l')]), "ijklmnopqr");
    assert_eq!(at(vec![code(KeyCode::Right)]), "efghijklmn");
    assert_eq!(at(vec![key('l'), key('h')]), "abcdefghij");
    assert_eq!(at(vec![key('h')]), "abcdefghij");
    // The ends: `$` stops at the widest line's overhang (26 - 10), `0` is the start.
    assert_eq!(at(vec![key('$')]), "qrstuvwxyz");
    assert_eq!(at(vec![key('$'), key('0')]), "abcdefghij");
    // Past the end is held at the end.
    let many = vec![key('l'); 20];
    assert_eq!(at(many), "qrstuvwxyz");
}

#[test]
fn a_text_that_wraps_ignores_the_sideways_keys() {
    let (_d, p) = file("app_nohscroll", "a.t", "abcdefghijklmnopqrstuvwxyz\n");
    let (term, _) = run_events(
        TestPlugin::all(),
        &p,
        (10, 5),
        vec![key('l'), key('$'), code(KeyCode::Right)],
    );
    let r = rows(&term);
    assert_eq!(r[0], "abcdefghij");
    assert_eq!(r[1], "klmnopqrst");
    assert_eq!(r[2], "uvwxyz");
}

#[test]
fn the_sideways_keys_are_named_only_while_they_act() {
    let named = |wrap: bool| {
        let k = AppKeys::defaults().with_wrap(wrap);
        k.help_sections()
            .iter()
            .flat_map(|s| s.rows.iter())
            .any(|(_, d)| d.contains("horizontal"))
    };
    assert!(!named(true), "wrapping: nothing to scroll sideways");
    assert!(named(false));
}

#[test]
fn a_text_file_that_cannot_be_read_any_more_is_shown_as_a_line_not_a_crash() {
    let (_d, p) = file("app_gone", "a.t", "x\n");
    let mut opened =
        open_file(vec![Arc::new(TestPlugin::all())], &p, "en", TermCaps::new()).unwrap();
    std::fs::remove_file(&p).unwrap();
    let mut term = Terminal::new(TestBackend::new(60, 3)).unwrap();
    let mut it = Vec::<AppEvent>::new().into_iter();
    run_with(
        &mut term,
        &mut *opened.session,
        &mut it,
        &cx(),
        &AppKeys::defaults(),
    )
    .unwrap();
    assert!(
        rows(&term)[0].starts_with("[can not read"),
        "{:?}",
        rows(&term)
    );
}

#[test]
fn every_sideways_row_the_help_names_acts_when_the_text_does_not_wrap() {
    let k = AppKeys::defaults().with_wrap(false);
    let mut seen = 0;
    for sec in k.help_sections() {
        for (keys_text, desc) in &sec.rows {
            if !desc.contains("horizontal") && !desc.contains("line start") {
                continue;
            }
            for part in keys_text.split(" / ").flat_map(|p| p.split(' ')) {
                let key = match part {
                    "\u{2190}" => Key::new(KeyCode::Left),
                    "\u{2192}" => Key::new(KeyCode::Right),
                    c if c.chars().count() == 1 => Key::char(c.chars().next().unwrap()),
                    other => panic!("an unexpected key {other:?} in `{keys_text}`"),
                };
                assert!(k.acts_on(&key, false, true), "{key:?} for `{desc}`");
                seen += 1;
            }
        }
    }
    // h l <- -> and 0 $.
    assert_eq!(seen, 6);
}
