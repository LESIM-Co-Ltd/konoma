//! The keys of the app come from the key layers shared with the host, and the user's `[keys]`
//! table changes what they do. Everything here is in memory: the config is a string parsed with
//! `SharedConfig::parse`, no file and no HOME is read.

use std::sync::Arc;

use konoma_host_core::shared::SharedConfig;
use konoma_plugin::app::{open_file, run_with, AppCx, AppEvent, AppKeys};
use konoma_plugin::prelude::*;
use konoma_plugin::TermCaps;
use konoma_test_support::unique_tmp;
use ratatui::backend::TestBackend;
use ratatui::Terminal;

use crate::plugin::TestPlugin;

fn keys(config: &str) -> AppKeys {
    let c = SharedConfig::parse(config).expect("the test config parses");
    AppKeys::new(&c.keys, &c.ui.keys)
}

fn lines(n: usize) -> String {
    (0..n).map(|i| format!("line {i:02}\n")).collect()
}

fn key(c: char) -> AppEvent {
    AppEvent::Key(Key::char(c))
}

fn ctrl(c: char) -> AppEvent {
    AppEvent::Key(Key::ctrl(c))
}

fn code(k: KeyCode) -> AppEvent {
    AppEvent::Key(Key::new(k))
}

/// Run the app over a 40-line text on a 24x6 screen (a page is 5 rows); returns the first body
/// row and how many events were left unread.
fn run(k: &AppKeys, events: Vec<AppEvent>) -> (String, usize) {
    let dir = unique_tmp("app_keys");
    std::fs::create_dir_all(dir.as_path()).unwrap();
    let p = dir.as_path().join("a.t");
    std::fs::write(&p, lines(40)).unwrap();
    let mut opened =
        open_file(vec![Arc::new(TestPlugin::all())], &p, "en", TermCaps::new()).expect("opens");
    let mut term = Terminal::new(TestBackend::new(24, 6)).unwrap();
    let cx = AppCx {
        lang: "en".into(),
        term: TermCaps::new(),
        title: "title".into(),
    };
    let mut it = events.into_iter();
    run_with(&mut term, &mut *opened.session, &mut it, &cx, k).expect("runs");
    let buf = term.backend().buffer();
    let first: String = (0..buf.area.width)
        .map(|x| buf[(x, 0)].symbol().to_string())
        .collect::<String>()
        .trim_end()
        .to_string();
    (first, it.count())
}

#[test]
fn the_defaults_move_page_and_jump() {
    let k = AppKeys::defaults();
    assert_eq!(run(&k, vec![key('j')]).0, "line 01");
    assert_eq!(run(&k, vec![key('j'), key('k')]).0, "line 00");
    assert_eq!(run(&k, vec![code(KeyCode::PageDown)]).0, "line 05");
    assert_eq!(run(&k, vec![ctrl('f')]).0, "line 05");
    assert_eq!(run(&k, vec![ctrl('f'), ctrl('b')]).0, "line 00");
    assert_eq!(run(&k, vec![ctrl('d')]).0, "line 02");
    assert_eq!(run(&k, vec![ctrl('d'), ctrl('u')]).0, "line 00");
    assert_eq!(run(&k, vec![key('G')]).0, "line 35");
    assert_eq!(run(&k, vec![key('G'), key('g')]).0, "line 00");
}

#[test]
fn q_and_capital_q_and_ctrl_c_leave() {
    let k = AppKeys::defaults();
    for ev in [key('q'), key('Q'), ctrl('c')] {
        assert_eq!(run(&k, vec![ev.clone(), key('j')]).1, 1, "{ev:?}");
    }
}

#[test]
fn space_is_the_menu_key_and_does_nothing_here() {
    let k = AppKeys::defaults();
    let (first, left) = run(&k, vec![key(' ')]);
    assert_eq!(first, "line 00");
    assert_eq!(left, 0);
}

#[test]
fn the_arrows_home_and_end_are_fixed_keys() {
    // Not in the keymap: no `[keys]` table reaches them.
    let k = keys(
        "[keys.text]\nj = \"noop\"\nk = \"noop\"\ng = \"noop\"\nG = \"noop\"\n[keys.global]\nq = \"noop\"\n",
    );
    assert_eq!(run(&k, vec![code(KeyCode::Down)]).0, "line 01");
    assert_eq!(
        run(&k, vec![code(KeyCode::Down), code(KeyCode::Up)]).0,
        "line 00"
    );
    assert_eq!(run(&k, vec![code(KeyCode::End)]).0, "line 35");
    assert_eq!(
        run(&k, vec![code(KeyCode::End), code(KeyCode::Home)]).0,
        "line 00"
    );
}

#[test]
fn a_rebind_moves_the_key() {
    let k = keys("[keys.text]\nx = \"navigate:down\"\n");
    assert_eq!(run(&k, vec![key('x'), key('x')]).0, "line 02");
    assert_eq!(
        run(&AppKeys::defaults(), vec![key('x')]).0,
        "line 00",
        "not a default key"
    );
    assert_eq!(
        run(&k, vec![key('j')]).0,
        "line 01",
        "the old key still works"
    );
}

#[test]
fn an_unbound_key_stops_acting() {
    let k = keys("[keys.text]\nj = \"noop\"\n");
    assert_eq!(run(&k, vec![key('j'), key('j')]).0, "line 00");
    assert_eq!(
        run(&k, vec![key('k'), key('G')]).0,
        "line 35",
        "the others are untouched"
    );
}

#[test]
fn the_leave_key_can_be_moved_and_removed() {
    // `q` is the context's own key, `Q` is layer 0: unbinding the first leaves the second.
    let k = keys("[keys.text]\nq = \"noop\"\nx = \"preview_back\"\n");
    assert_eq!(run(&k, vec![key('q'), key('j')]).1, 0, "q no longer leaves");
    assert_eq!(run(&k, vec![key('x'), key('j')]).1, 1, "x leaves now");
    assert_eq!(run(&k, vec![key('Q'), key('j')]).1, 1, "Q still leaves");
}

#[test]
fn the_global_table_reaches_the_app() {
    let k = keys("[keys.global]\nX = \"quit\"\nQ = \"noop\"\n");
    assert_eq!(run(&k, vec![key('X'), key('j')]).1, 1);
    assert_eq!(run(&k, vec![key('Q'), key('j')]).1, 0);
}

#[test]
fn a_context_cannot_take_a_global_key_for_another_command() {
    let k = keys("[keys.text]\nQ = \"navigate:down\"\n");
    assert_eq!(run(&k, vec![key('Q'), key('j')]).1, 1, "Q still quits");
}

#[test]
fn ctrl_c_leaves_whatever_the_config_says() {
    let k = keys("[keys.text]\nq = \"noop\"\n[keys.global]\nQ = \"noop\"\n");
    assert_eq!(run(&k, vec![ctrl('c'), key('j')]).1, 1);
}

#[test]
fn the_less_style_pages_with_plain_letters() {
    let k = keys("[ui]\nkeys = \"less\"\n");
    assert_eq!(run(&k, vec![key('f')]).0, "line 05");
    assert_eq!(run(&k, vec![key('f'), key('b')]).0, "line 00");
    assert_eq!(run(&k, vec![key('d')]).0, "line 02");
    assert_eq!(
        run(&k, vec![ctrl('f')]).0,
        "line 00",
        "Ctrl-f is the vim style"
    );
    // And the vim style does not take `f`.
    assert_eq!(run(&AppKeys::defaults(), vec![key('f')]).0, "line 00");
}

#[test]
fn commands_the_app_cannot_do_yet_are_ignored() {
    // `e` edits, `o` opens, `+` zooms, `J` is the next page: shared keys, but nothing happens.
    let k = AppKeys::defaults();
    let (first, left) = run(
        &k,
        vec![
            key('e'),
            key('o'),
            key('+'),
            key('J'),
            key('y'),
            key('t'),
            key('?'),
        ],
    );
    assert_eq!(first, "line 00");
    assert_eq!(left, 0, "none of them ended the loop");
}

#[test]
fn a_host_only_command_in_the_config_is_ignored() {
    let k = keys("[keys.text]\nx = \"search_start\"\nz = \"no_such_action\"\n");
    assert_eq!(run(&k, vec![key('x'), key('z'), key('j')]).0, "line 01");
}

#[test]
fn a_shared_command_the_app_cannot_do_may_still_be_bound() {
    // `page_next` is a shared command with no meaning here: binding it changes nothing.
    let k = keys("[keys.text]\nj = \"page_next\"\n");
    assert_eq!(run(&k, vec![key('j')]).0, "line 00");
}

#[test]
fn the_old_context_names_are_read() {
    let k = keys("[keys.preview_text]\nx = \"navigate:down\"\n");
    assert_eq!(run(&k, vec![key('x')]).0, "line 01");
}

#[test]
fn a_broken_key_name_in_the_config_is_ignored() {
    let k = keys("[keys.text]\n\"ctrl-\" = \"navigate:down\"\n\"ab\" = \"navigate:down\"\n");
    assert_eq!(run(&k, vec![key('j')]).0, "line 01");
}

#[test]
fn a_broken_config_is_reported_on_one_line_and_the_defaults_keys_work() {
    use konoma_plugin::app::{keys_from_load, write_load_error};
    let dir = unique_tmp("app_keys_cfg");
    std::fs::create_dir_all(dir.as_path()).unwrap();
    let cfg = dir.as_path().join("config.toml");
    std::fs::write(
        &cfg,
        "[keys.text
j = \"noop\"\n",
    )
    .unwrap();
    let (k, err) = keys_from_load(SharedConfig::load(&cfg));
    let mut out = Vec::new();
    write_load_error(&mut out, err.as_deref()).unwrap();
    let text = String::from_utf8(out).unwrap();
    assert!(
        text.starts_with("config error (using defaults): "),
        "{text}"
    );
    assert_eq!(text.lines().count(), 1, "{text:?}");
    assert!(text.ends_with('\n'));
    // The file is still shown, with the default keys (`j` moves).
    assert_eq!(run(&k, vec![key('j')]).0, "line 01");
}

#[test]
fn no_config_error_prints_nothing() {
    use konoma_plugin::app::{keys_from_load, write_load_error};
    let dir = unique_tmp("app_keys_cfg_ok");
    std::fs::create_dir_all(dir.as_path()).unwrap();
    let good = dir.as_path().join("config.toml");
    std::fs::write(&good, "[keys.text]\nx = \"navigate:down\"\n").unwrap();
    for path in [good, dir.as_path().join("missing.toml")] {
        let (_, err) = keys_from_load(SharedConfig::load(&path));
        let mut out = Vec::new();
        write_load_error(&mut out, err.as_deref()).unwrap();
        assert!(
            out.is_empty(),
            "{path:?}: {:?}",
            String::from_utf8_lossy(&out)
        );
    }
}

#[test]
fn under_less_f_pages_forward_and_space_is_only_the_menu() {
    let k = keys("[ui]\nkeys = \"less\"\n");
    assert_eq!(run(&k, vec![key('f')]).0, "line 05");
    assert_eq!(run(&k, vec![key('f'), key('f')]).0, "line 10");
    let (first, left) = run(&k, vec![key(' ')]);
    assert_eq!(
        (first.as_str(), left),
        ("line 00", 0),
        "Space does not page"
    );
}
