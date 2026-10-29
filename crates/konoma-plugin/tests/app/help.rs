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
fn run(k: &AppKeys, size: (u16, u16), events: Vec<AppEvent>) -> (Vec<String>, usize) {
    run_as(k, "a.t", size, events)
}

/// [`run`] over the file `name` (its extension picks the view: see `plugin.rs`).
fn run_as(
    k: &AppKeys,
    name: &str,
    (w, h): (u16, u16),
    events: Vec<AppEvent>,
) -> (Vec<String>, usize) {
    let dir = unique_tmp("app_help");
    std::fs::create_dir_all(dir.as_path()).unwrap();
    let p = dir.as_path().join(name);
    let body: String = (0..40).map(|i| format!("line {i:02}\n")).collect();
    std::fs::write(&p, body).unwrap();
    let mut opened =
        open_file(vec![Arc::new(TestPlugin::all())], &p, "en", TermCaps::new()).expect("opens");
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    let cx = AppCx {
        lang: "en".into(),
        term: TermCaps::new(),
        title: "title".into(),
        ..AppCx::default()
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
        "[keys.text]\nx = \"toggle_help\"\n[keys.global]\n\"?\" = \"noop\"\n",
        Lang::En,
    );
    let (r, _) = run(&k, (70, 3), vec![]);
    assert!(r[2].contains(" x:help"), "{r:?}");
    assert!(!r[2].contains("?"), "{r:?}");
}

#[test]
fn the_less_scheme_names_its_own_keys() {
    let k = keys("[ui]\nkeys = \"less\"\n", Lang::En);
    let (r, _) = run(&k, (80, 3), vec![]);
    assert_eq!(r[2], " q:quit  j/k:scroll  ?:help  line 1  title");
    let (r, _) = run(&k, (80, 24), vec![key('?')]);
    let body = [
        "Preview: text / Markdown",
        "  g / G                        top / bottom",
        "  f / b  \u{b7}  PageDown / PageUp  page",
        "  d / u                        half page",
        "  q / Esc                      quit",
        "",
        "Movement",
        "  j / k / \u{2191} \u{2193}                  scroll",
        "",
        "Global",
        "  ?                            toggle this help",
        "  Q                            quit",
    ];
    assert_eq!(
        r,
        screen(&body, " Help  (?/Esc to close, j/k to scroll)", 24)
    );
    // Vim's page keys are not the less scheme's: `Ctrl-f` names nothing here.
    assert!(!r.join("\n").contains("Ctrl-f"));
}

// ---------------------------------------------------------------------------------------------
// The title of the help is made from the bindings.
// ---------------------------------------------------------------------------------------------

/// The last row of the screen with the help open (its title).
fn title_of(config: &str, lang: Lang) -> String {
    // Opened with `x` when the config rebinds it (the default `?` otherwise).
    let opener = if config.contains("x = \"toggle_help\"") {
        'x'
    } else {
        '?'
    };
    let (r, _) = run(&keys(config, lang), (70, 6), vec![key(opener)]);
    r[5].clone()
}

#[test]
fn the_help_title_names_the_keys_that_close_and_scroll_it() {
    // Rebound: `x` toggles the help and `?` does not, so the title names `x`.
    let c = "[keys.text]\nx = \"toggle_help\"\n[keys.global]\n\"?\" = \"noop\"\n";
    assert_eq!(
        title_of(c, Lang::En),
        " Help  (x/Esc to close, j/k to scroll)"
    );
    // ... and `x` really closes it, while `?` does nothing (it neither opens nor closes).
    let k = keys(c, Lang::En);
    let (r, _) = run(&k, (70, 6), vec![key('x'), key('?')]);
    assert!(r[5].contains("Help"), "`?` did not close the help: {r:?}");
    let (r, _) = run(&k, (70, 6), vec![key('x'), key('x')]);
    assert!(r[5].contains("q:quit"), "`x` closed the help: {r:?}");
    let (r, _) = run(&k, (70, 6), vec![key('?')]);
    assert!(r[5].contains("q:quit"), "`?` did not open it: {r:?}");
    // Both keys close it.
    let c = "[keys.text]\nx = \"toggle_help\"\n";
    assert_eq!(
        title_of(c, Lang::En),
        " Help  (?/x/Esc to close, j/k to scroll)"
    );
    // Unbound `j`: only `k` scrolls; both unbound: the clause is gone.
    assert_eq!(
        title_of("[keys.text]\nj = \"noop\"\n", Lang::En),
        " Help  (?/Esc to close, k to scroll)"
    );
    assert_eq!(
        title_of("[keys.text]\nj = \"noop\"\nk = \"noop\"\n", Lang::En),
        " Help  (?/Esc to close)"
    );
    // A key rebound to move down is named.
    assert_eq!(
        title_of("[keys.text]\nH = \"navigate:down\"\n", Lang::En),
        " Help  (?/Esc to close, H/k/j to scroll)"
    );
    // Japanese, with the same keys.
    assert_eq!(
        title_of("[keys.text]\nj = \"noop\"\nk = \"noop\"\n", Lang::Jp),
        " ヘルプ  (?/Esc で閉じる)"
    );
    assert_eq!(
        title_of(
            "[keys.text]\nx = \"toggle_help\"\n[keys.global]\n\"?\" = \"noop\"\n",
            Lang::Jp
        ),
        " ヘルプ  (x/Esc で閉じる, j/k スクロール)"
    );
}

#[test]
fn the_help_title_follows_the_global_table_and_the_scheme_too() {
    // A global rebinding of the help key reaches the text context.
    assert_eq!(
        title_of("[keys.global]\nw = \"toggle_help\"\n", Lang::En),
        " Help  (?/w/Esc to close, j/k to scroll)"
    );
    // The title is the one `AppKeys` reports.
    let k = keys("[keys.text]\nj = \"noop\"\n", Lang::En);
    assert_eq!(k.help_title(), " Help  (?/Esc to close, k to scroll) ");
}

// ---------------------------------------------------------------------------------------------
// `Q` with the help open: the host's `Q` quits (its confirmation aside), so does the app's.
// ---------------------------------------------------------------------------------------------

#[test]
fn q_with_the_help_open_quits_the_app_like_the_host() {
    let k = keys("", Lang::En);
    let (_, left) = run(&k, (70, 6), vec![key('?'), key('Q'), key('x')]);
    assert_eq!(left, 1, "`Q` ended the loop from the help");
    // With `q` unbound `Q` still quits from the help.
    let k = keys("[keys.text]\nq = \"noop\"\n", Lang::En);
    let (_, left) = run(&k, (70, 6), vec![key('?'), key('Q'), key('x')]);
    assert_eq!(left, 1);
    // `q` unbound: it does nothing in the help (the help stays), `Esc` still closes it.
    let (r, left) = run(&k, (70, 6), vec![key('?'), key('q'), key('x')]);
    assert_eq!(left, 0);
    assert!(r[5].contains("Help"), "{r:?}");
    let (r, left) = run(
        &k,
        (70, 6),
        vec![key('?'), AppEvent::Key(Key::new(KeyCode::Esc)), key('x')],
    );
    assert_eq!(left, 0, "x is read after the close");
    assert!(!r[5].contains("Help"), "Esc closed the help: {r:?}");
    // `Q` unbound as well: it does nothing in the help.
    let k = keys("[keys.global]\nQ = \"noop\"\n", Lang::En);
    let (r, left) = run(&k, (70, 6), vec![key('?'), key('Q')]);
    assert_eq!(left, 0);
    assert!(r[5].contains("Help"), "{r:?}");
    // Ctrl-C quits from the help whatever is bound.
    let (_, left) = run(
        &k,
        (70, 6),
        vec![key('?'), AppEvent::Key(Key::ctrl('c')), key('x')],
    );
    assert_eq!(left, 1);
}

// ---------------------------------------------------------------------------------------------
// Widths are cells, not characters.
// ---------------------------------------------------------------------------------------------

#[test]
fn a_full_width_key_aligns_the_help_by_cells() {
    // `あ` is two cells wide; the config accepts any one-character key.
    let k = keys("[keys.text]\n\"あ\" = \"navigate:down\"\n", Lang::En);
    let (r, _) = run(&k, (70, 20), vec![key('?')]);
    let w = |s: &str| unicode_width::UnicodeWidthStr::width(s);
    let rows: Vec<&String> = r.iter().filter(|l| l.starts_with("  ")).collect();
    assert!(rows.iter().any(|l| l.contains("あ")), "{r:?}");
    // Every description starts in the same cell column.
    // (the longest first: `half page` ends in `page`)
    let descs = [
        "toggle this help",
        "top / bottom",
        "half page",
        "scroll",
        "page",
        "quit",
    ];
    let cols: Vec<usize> = rows
        .iter()
        .filter_map(|l| {
            descs
                .iter()
                .find_map(|d| l.rfind(d).filter(|_| l.ends_with(d)).map(|i| w(&l[..i])))
        })
        .collect();
    assert_eq!(cols.len(), rows.len(), "{rows:?}");
    assert!(cols.windows(2).all(|p| p[0] == p[1]), "{cols:?} {rows:?}");
    // The key is bound, so the footer's hint names it and it scrolls.
    let (r, _) = run(&k, (70, 3), vec![key('あ')]);
    assert_eq!(r[0], "line 01");
    assert!(r[2].contains("あ"), "{r:?}");
}

// ---------------------------------------------------------------------------------------------
// A hint is there only when its key acts in the frame on screen.
// ---------------------------------------------------------------------------------------------

#[test]
fn the_footer_names_scrolling_only_for_a_frame_that_scrolls() {
    let k = keys("", Lang::En);
    // Text in each form scrolls.
    for name in ["a.t", "a.o", "a.l"] {
        let (r, _) = run_as(&k, name, (70, 3), vec![]);
        assert_eq!(r[2], " q:quit  j/k:scroll  ?:help  line 1  title", "{name}");
        let (r, _) = run_as(&k, name, (70, 3), vec![key('j')]);
        assert_eq!(r[0], "line 01", "{name}: `j` scrolled");
    }
    // A canvas, a message, a view the app cannot show and a failure do not: no hint, no
    // position, and the key does nothing (the screen is the one before).
    for name in ["a.c", "a.big", "a.m", "a.list", "a.fail"] {
        let (before, _) = run_as(&k, name, (70, 5), vec![]);
        assert_eq!(
            before[4], " q:quit  ?:help  title",
            "{name}: no scroll hint, no line"
        );
        for press in ["j", "G", "k", "g"] {
            let ev = vec![key(press.chars().next().unwrap())];
            let (after, left) = run_as(&k, name, (70, 5), ev);
            assert_eq!(left, 0);
            assert_eq!(after, before, "{name}: `{press}` changed the screen");
        }
        // The help still opens and scrolls (it is its own frame).
        let (r, _) = run_as(&k, name, (70, 6), vec![key('?'), key('j')]);
        assert_eq!(
            r[0], "  g / G                                  top / bottom",
            "{name}"
        );
    }
}

// ---------------------------------------------------------------------------------------------
// Property: every key the app names, in the footer or in the help, acts.
// ---------------------------------------------------------------------------------------------

/// A key as the footer writes it, back to a key.
pub(crate) fn parse_footer_key(s: &str) -> Key {
    if let Some(rest) = s.strip_prefix("C-") {
        let mut it = rest.chars();
        let c = it.next().expect("a key after C-");
        assert!(it.next().is_none(), "Ctrl-chord of one character: {s}");
        return Key::ctrl(c);
    }
    named_or_char(s, &[("PgDn", KeyCode::PageDown), ("PgUp", KeyCode::PageUp)])
}

/// A key as the help writes it, back to a key.
pub(crate) fn parse_help_key(s: &str) -> Key {
    if let Some(rest) = s.strip_prefix("Ctrl-") {
        return parse_help_key_plain(rest, true);
    }
    parse_help_key_plain(s, false)
}

fn parse_help_key_plain(s: &str, ctrl: bool) -> Key {
    let k = named_or_char(
        s,
        &[("PageDown", KeyCode::PageDown), ("PageUp", KeyCode::PageUp)],
    );
    if ctrl {
        match k.code {
            KeyCode::Char(c) => Key::ctrl(c),
            other => panic!("a Ctrl-chord of {other:?}"),
        }
    } else {
        k
    }
}

fn named_or_char(s: &str, extra: &[(&str, KeyCode)]) -> Key {
    if let Some((_, c)) = extra.iter().find(|(n, _)| *n == s) {
        return Key::new(c.clone());
    }
    let code = match s {
        "Space" => KeyCode::Char(' '),
        "Enter" | "\u{21b5}" => KeyCode::Enter,
        "Tab" => KeyCode::Tab,
        "\u{21e7}Tab" => KeyCode::BackTab,
        "Esc" => KeyCode::Esc,
        "Home" => KeyCode::Home,
        "End" => KeyCode::End,
        "\u{2191}" => KeyCode::Up,
        "\u{2193}" => KeyCode::Down,
        "\u{2190}" => KeyCode::Left,
        "\u{2192}" => KeyCode::Right,
        _ => {
            let mut it = s.chars();
            let c = it.next().unwrap_or_else(|| panic!("an empty key"));
            assert!(it.next().is_none(), "unparsed key notation `{s}`");
            KeyCode::Char(c)
        }
    };
    Key::new(code)
}

/// The keys of a footer token's text (`j/k:scroll` -> `j`, `k`).
pub(crate) fn token_keys(text: &str) -> Vec<Key> {
    let (keys, _label) = text.rsplit_once(':').expect("a token is keys:label");
    keys.split('/').map(parse_footer_key).collect()
}

/// The keys of a help row's key column (`j / k / \u{2191} \u{2193}`, `Ctrl-f / Ctrl-b  \u{b7}  PageDown`).
pub(crate) fn row_keys(text: &str) -> Vec<Key> {
    text.split_whitespace()
        .filter(|p| *p != "/" && *p != "\u{b7}")
        .map(parse_help_key)
        .collect()
}

/// The configs to try: (name, config).
fn property_configs() -> Vec<(&'static str, &'static str)> {
    vec![
        ("default", ""),
        (
            "rebind",
            "[keys.text]\nx = \"toggle_help\"\nH = \"navigate:down\"\n",
        ),
        (
            "rebind help away",
            "[keys.text]\nx = \"toggle_help\"\n[keys.global]\n\"?\" = \"noop\"\n",
        ),
        ("unbind j", "[keys.text]\nj = \"noop\"\n"),
        ("unbind j k", "[keys.text]\nj = \"noop\"\nk = \"noop\"\n"),
        ("unbind q", "[keys.text]\nq = \"noop\"\n"),
        (
            "unbind q Q",
            "[keys.text]\nq = \"noop\"\n[keys.global]\nQ = \"noop\"\n",
        ),
        ("global rebind", "[keys.global]\nw = \"toggle_help\"\n"),
        ("full width", "[keys.text]\n\"あ\" = \"navigate:down\"\n"),
        ("less", "[ui]\nkeys = \"less\"\n"),
        (
            "less unbind",
            "[ui]\nkeys = \"less\"\n[keys.text]\nj = \"noop\"\nd = \"noop\"\n",
        ),
        (
            "page keys rebound",
            "[keys.text]\n\"n\" = \"navigate:page_down\"\n",
        ),
    ]
}

#[test]
fn every_key_the_footer_and_the_help_name_acts() {
    let mut checked_tokens = 0;
    let mut checked_rows = 0;
    for (name, cfg) in property_configs() {
        for lang in [Lang::En, Lang::Jp] {
            let k = keys(cfg, lang);
            // The footer, per kind of frame.
            for scrolls in [true, false] {
                let hints = k.footer_hints(scrolls);
                assert!(!hints.is_empty(), "{name}: a footer always has the way out");
                for t in &hints {
                    let parsed = token_keys(&t.text);
                    assert_eq!(parsed.len(), t.keys.len(), "{name} {}: {t:?}", t.text);
                    for key in parsed {
                        assert!(
                            k.acts_on(&key, false, scrolls),
                            "{name} {lang:?} scrolls={scrolls}: the footer names {key:?} in `{}` but it does nothing",
                            t.text
                        );
                        checked_tokens += 1;
                    }
                }
            }
            // The help rows: in the help every motion acts (it scrolls the help).
            for sec in k.help_sections() {
                for (keys_text, desc) in &sec.rows {
                    for key in row_keys(keys_text) {
                        assert!(
                            k.acts_on(&key, true, true),
                            "{name} {lang:?}: the help names {key:?} for `{desc}` but it does nothing"
                        );
                        checked_rows += 1;
                    }
                }
            }
        }
    }
    // Not vacuous.
    assert!(checked_tokens > 60, "{checked_tokens}");
    assert!(checked_rows > 150, "{checked_rows}");
}

/// Every kind of frame the test plugin can put on screen: (file, whether it scrolls).
const FRAMES: [(&str, bool); 8] = [
    ("a.t", true),
    ("a.o", true),
    ("a.l", true),
    ("a.c", false),
    ("a.big", false),
    ("a.m", false),
    ("a.list", false),
    ("a.fail", false),
];

const REAL_SIZE: (u16, u16) = (100, 6);

/// What a real run left: the rows of the last frame and whether the loop ended (`quit`: a
/// trailing `Resize` was left unread).
fn observe(
    k: &AppKeys,
    name: &str,
    setup: &[AppEvent],
    pressed: Option<&Key>,
) -> (Vec<String>, bool) {
    let mut ev: Vec<AppEvent> = setup.to_vec();
    if let Some(p) = pressed {
        ev.push(AppEvent::Key(p.clone()));
    }
    ev.push(AppEvent::Resize);
    let (rows, left) = run_as(k, name, REAL_SIZE, ev);
    assert!(left <= 1, "{left} events left");
    (rows, left == 1)
}

/// The key that opens the help, as the footer names it (`None` when none does).
fn help_opener(k: &AppKeys) -> Option<Key> {
    let hint = k.footer_hints(false).pop()?;
    let key = token_keys(&hint.text).into_iter().next()?;
    // It must really be the help: it opens it on a real frame.
    let (closed, _) = observe(k, "a.t", &[], None);
    let (open, quit) = observe(k, "a.t", &[], Some(&key));
    (!quit && open != closed).then_some(key)
}

/// Whether `key` really acts in the loop on `name`: the screen differs from the one without the
/// key, or the loop ends. The screen is first moved off its edges (three `Down` presses: fixed
/// keys, in the help or not) so a motion that acts changes it whichever way it goes.
fn acts_in_the_loop(k: &AppKeys, name: &str, help: Option<&Key>, key: &Key) -> bool {
    let mut setup: Vec<AppEvent> = Vec::new();
    if let Some(h) = help {
        setup.push(AppEvent::Key(h.clone()));
    }
    for _ in 0..3 {
        setup.push(AppEvent::Key(Key::new(KeyCode::Down)));
    }
    let (before, before_quit) = observe(k, name, &setup, None);
    assert!(!before_quit, "the set-up alone ends the loop");
    let (after, quit) = observe(k, name, &setup, Some(key));
    quit || after != before
}

/// Whether the footer on a real screen has the scroll form (a position after the hints).
fn footer_scrolls(k: &AppKeys, rows: &[String], title: &str) -> bool {
    let plain = format!(" {}  {title}", k.footer_tokens(false).join("  "));
    *rows.last().expect("a footer row") != plain
}

#[test]
fn what_acts_on_a_frame_is_what_the_loop_does() {
    // `acts_on` is the property's oracle, so pin it to the loop on every kind of frame: it
    // says true exactly when the real loop changes the screen (or ends), in the help or not.
    let mut extra: Vec<Key> = "jkgGxeozdufbn \u{3042}HQq?"
        .chars()
        .map(Key::char)
        .collect();
    for c in ['f', 'b', 'd', 'u', 'c'] {
        extra.push(Key::ctrl(c));
    }
    for code in [
        KeyCode::Down,
        KeyCode::Up,
        KeyCode::Home,
        KeyCode::End,
        KeyCode::Esc,
        KeyCode::Enter,
        KeyCode::Tab,
        KeyCode::PageDown,
        KeyCode::PageUp,
        KeyCode::Left,
        KeyCode::Right,
    ] {
        extra.push(Key::new(code));
    }
    let (mut checked, mut true_cases, mut false_cases, mut help_cases) = (0, 0, 0, 0);
    for (name, cfg) in property_configs() {
        let k = keys(cfg, Lang::En);
        let opener = help_opener(&k);
        // The keys to try: the extras and every key the footer and the help name.
        let mut cands = extra.clone();
        for t in k.footer_hints(true) {
            cands.extend(token_keys(&t.text));
        }
        for sec in k.help_sections() {
            for (keys_text, _) in &sec.rows {
                cands.extend(row_keys(keys_text));
            }
        }
        let mut seen = std::collections::BTreeSet::new();
        cands.retain(|c| seen.insert(format!("{c:?}")));
        for (frame, scrolls) in FRAMES {
            for key in &cands {
                let mut states: Vec<(bool, Option<&Key>)> = vec![(false, None)];
                if let Some(o) = &opener {
                    states.push((true, Some(o)));
                }
                for (in_help, opener) in states {
                    let oracle = k.acts_on(key, in_help, scrolls);
                    let real = acts_in_the_loop(&k, frame, opener, key);
                    assert_eq!(
                        oracle, real,
                        "{name} {frame} help={in_help}: acts_on({key:?}) = {oracle} but the loop {}",
                        if real { "acts" } else { "does nothing" }
                    );
                    checked += 1;
                    help_cases += usize::from(in_help);
                    true_cases += usize::from(real);
                    false_cases += usize::from(!real);
                }
            }
        }
    }
    // Not vacuous: both outcomes, in and out of the help, over many cases.
    assert!(checked > 3000, "{checked}");
    assert!(true_cases > 500, "{true_cases}");
    assert!(false_cases > 500, "{false_cases}");
    assert!(help_cases > 1000, "{help_cases}");
    // The oracle's own fixed points.
    let k = keys("", Lang::En);
    assert!(k.acts_on(&Key::char('j'), false, true));
    assert!(!k.acts_on(&Key::char('j'), false, false));
    assert!(k.acts_on(&Key::char('j'), true, false));
    assert!(k.acts_on(&Key::char('q'), false, false));
    assert!(
        !k.acts_on(&Key::char('e'), true, true),
        "`e` edits nothing here"
    );
}

#[test]
fn what_the_footer_names_on_a_real_frame_acts_in_the_loop() {
    // The property over real frames: `scrolls` is read off the screen the loop drew (not
    // passed in), then every key the footer names there must act in the loop.
    let mut checked = 0;
    for (name, cfg) in property_configs() {
        for lang in [Lang::En, Lang::Jp] {
            let k = keys(cfg, lang);
            for (frame, expect_scrolls) in FRAMES {
                let (rows, _) = run_as(&k, frame, REAL_SIZE, vec![]);
                let scrolls = footer_scrolls(&k, &rows, "title");
                assert_eq!(scrolls, expect_scrolls, "{name} {lang:?} {frame}: {rows:?}");
                for t in k.footer_hints(scrolls) {
                    for key in token_keys(&t.text) {
                        assert!(
                            acts_in_the_loop(&k, frame, None, &key),
                            "{name} {lang:?} {frame}: the footer names {key:?} in `{}` but the loop does nothing",
                            t.text
                        );
                        checked += 1;
                    }
                }
            }
        }
    }
    assert!(checked > 400, "{checked}");
}
