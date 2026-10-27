//! The footer and the `?` help of the app come from the builders shared with the host
//! (`konoma_host_core::help`): the labels follow the real bindings and the language follows
//! `ui.lang`. In memory only: the config is a string, no file and no HOME is read.

use std::sync::Arc;

use konoma_host_core::lang::Lang;
use konoma_host_core::shared::SharedConfig;
use konoma_plugin::app::{open_file, run_with, AppCx, AppEvent, AppKeys};
use konoma_plugin::prelude::*;
use konoma_plugin::TermCaps;
use konoma_test_support::unique_tmp;
use ratatui::backend::TestBackend;
use ratatui::Terminal;

use crate::plugin::TestPlugin;

fn keys(config: &str, lang: Lang) -> AppKeys {
    let c = SharedConfig::parse(config).expect("the test config parses");
    AppKeys::new(&c.keys, &c.ui.keys).with_lang(lang)
}

fn key(c: char) -> AppEvent {
    AppEvent::Key(Key::char(c))
}

/// Run the app over a 40-line text on a `w` x `h` screen; the rows of the last frame and how
/// many events were left unread.
fn run(k: &AppKeys, (w, h): (u16, u16), events: Vec<AppEvent>) -> (Vec<String>, usize) {
    let dir = unique_tmp("app_help");
    std::fs::create_dir_all(dir.as_path()).unwrap();
    let p = dir.as_path().join("a.t");
    let body: String = (0..40).map(|i| format!("line {i:02}\n")).collect();
    std::fs::write(&p, body).unwrap();
    let mut opened =
        open_file(vec![Arc::new(TestPlugin::all())], &p, "en", TermCaps::new()).expect("opens");
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    let cx = AppCx {
        lang: "en".into(),
        term: TermCaps::new(),
        title: "title".into(),
    };
    let mut it = events.into_iter();
    run_with(&mut term, &mut *opened.session, &mut it, &cx, k).expect("runs");
    let buf = term.backend().buffer();
    // A wide character covers the cell after it; that cell keeps what an earlier frame left in
    // it on the test backend, so it is skipped.
    let rows = (0..buf.area.height)
        .map(|y| {
            let mut row = String::new();
            let mut skip = 0;
            for x in 0..buf.area.width {
                if skip > 0 {
                    skip -= 1;
                    continue;
                }
                let s = buf[(x, y)].symbol();
                skip = unicode_width::UnicodeWidthStr::width(s).saturating_sub(1);
                row.push_str(s);
            }
            row.trim_end().to_string()
        })
        .collect();
    (rows, it.count())
}

/// The rows of a screen that shows `body` first and blank rows to the last, then `footer`.
fn screen(body: &[&str], footer: &str, h: usize) -> Vec<String> {
    let mut v: Vec<String> = body.iter().map(|s| s.to_string()).collect();
    v.resize(h - 1, String::new());
    v.push(footer.to_string());
    v
}

#[test]
fn the_footer_and_the_help_in_english() {
    let k = keys("", Lang::En);
    let (r, left) = run(&k, (70, 3), vec![]);
    assert_eq!(left, 0);
    assert_eq!(
        r,
        [
            "line 00",
            "line 01",
            " q:quit  j/k:scroll  ?:help  line 1  title"
        ]
    );
    let (r, left) = run(&k, (70, 20), vec![key('?')]);
    assert_eq!(left, 0);
    let body = [
        "Preview: text / Markdown",
        "  g / G                                  top / bottom",
        "  Ctrl-f / Ctrl-b  ·  PageDown / PageUp  page",
        "  Ctrl-d / Ctrl-u                        half page",
        "  q / Esc                                quit",
        "",
        "Movement",
        "  j / k / ↑ ↓                            scroll",
        "",
        "Global",
        "  ?                                      toggle this help",
        "  Q                                      quit",
    ];
    assert_eq!(
        r,
        screen(&body, " Help  (?/Esc to close, j/k to scroll)", 20)
    );
}

#[test]
fn the_footer_and_the_help_in_japanese() {
    let k = keys("", Lang::Jp);
    let (r, _) = run(&k, (70, 3), vec![]);
    assert_eq!(
        r,
        [
            "line 00",
            "line 01",
            " q:終了  j/k:スクロール  ?:ヘルプ  line 1  title"
        ]
    );
    let (r, _) = run(&k, (70, 20), vec![key('?')]);
    let body = [
        "プレビュー: テキスト/Markdown",
        "  g / G                                  先頭 / 末尾",
        "  Ctrl-f / Ctrl-b  ·  PageDown / PageUp  ページ",
        "  Ctrl-d / Ctrl-u                        半ページ",
        "  q / Esc                                終了",
        "",
        "移動 (Movement)",
        "  j / k / ↑ ↓                            スクロール",
        "",
        "共通 (Global)",
        "  ?                                      このヘルプの開閉",
        "  Q                                      終了",
    ];
    assert_eq!(
        r,
        screen(&body, " ヘルプ  (?/Esc で閉じる, j/k スクロール)", 20)
    );
}

#[test]
fn the_help_closes_with_question_esc_and_q_and_scrolls() {
    let k = keys("", Lang::En);
    for close in [key('?'), key('q'), AppEvent::Key(Key::new(KeyCode::Esc))] {
        // Closed: the text is back and the loop is still running (the next `q` leaves).
        let (r, left) = run(
            &k,
            (70, 6),
            vec![key('?'), close.clone(), key('j'), key('q'), key('x')],
        );
        assert_eq!(
            left, 1,
            "{close:?}: the last `q` ended the loop, not the close"
        );
        assert_eq!(r[0], "line 01", "{close:?}");
        assert!(r[5].contains("q:quit"), "{close:?}: {r:?}");
    }
    // `j` scrolls the help (the first row moves off), not the text.
    let (r, _) = run(&k, (70, 6), vec![key('?'), key('j')]);
    assert_eq!(
        r[0],
        "  g / G                                  top / bottom"
    );
    // Esc out of the help ends the app (the help row says `q / Esc` leaves).
    let (_, left) = run(
        &k,
        (70, 6),
        vec![AppEvent::Key(Key::new(KeyCode::Esc)), key('x')],
    );
    assert_eq!(left, 1);
    // Ctrl-C leaves from the help.
    let (_, left) = run(
        &k,
        (70, 6),
        vec![key('?'), AppEvent::Key(Key::ctrl('c')), key('x')],
    );
    assert_eq!(left, 1);
}

#[test]
fn unbinding_q_changes_the_footer_and_the_help() {
    // `q` gone: `Q` is what leaves, and the footer says so.
    let k = keys("[keys.text]\nq = \"noop\"\n", Lang::En);
    let (r, _) = run(&k, (70, 3), vec![]);
    assert_eq!(r[2], " Q:quit  j/k:scroll  ?:help  line 1  title");
    // Both gone: only Ctrl-C quits, and the footer says that rather than `q`.
    let k = keys(
        "[keys.text]\nq = \"noop\"\n[keys.global]\nQ = \"noop\"\n",
        Lang::En,
    );
    let (r, _) = run(&k, (70, 3), vec![]);
    assert_eq!(r[2], " C-c:quit  j/k:scroll  ?:help  line 1  title");
    let (r, _) = run(&k, (70, 20), vec![key('?')]);
    assert!(
        !r.iter().any(|l| l.contains("quit") && !l.contains("Esc")),
        "no row claims a key quits that does not: {r:?}"
    );
    // The key rebound to scroll is the one the footer names; `?` rebound is named too.
    let k = keys(
        "[keys.text]\nx = \"toggle_help\"\n\"?\" = \"noop\"\n",
        Lang::En,
    );
    let (r, _) = run(&k, (70, 3), vec![]);
    assert!(r[2].contains("x:help"), "{r:?}");
    assert!(!r[2].contains("?:help"), "{r:?}");
}

#[test]
fn the_less_scheme_names_its_own_keys() {
    let k = keys("[ui]\nkeys = \"less\"\n", Lang::En);
    let (r, _) = run(&k, (80, 24), vec![key('?')]);
    let text = r.join("\n");
    assert!(text.contains("f / b") || text.contains("Space"), "{text}");
}
