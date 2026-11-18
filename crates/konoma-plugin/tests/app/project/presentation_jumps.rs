//! The standalone project app's screen jumps are operations (`goto_<screen>`): `[keys.<plugin>]`
//! rebinds and removes them, and the tab bar and the help show the keys that work.

use super::{run, setup_with, text, Log};
use ratatui::{backend::TestBackend, Terminal};

const ID: &str = "project-presentation";

fn config(entries: &str) -> String {
    format!("[ui]\nlang='en'\n[keys.{ID}]\n{entries}")
}

fn jumps_log() -> Log {
    Log {
        jumps: true,
        ..Log::default()
    }
}

/// Row `y` of the terminal as text.
fn row(terminal: &Terminal<TestBackend>, y: u16) -> String {
    let buffer = terminal.backend().buffer();
    (0..buffer.area.width)
        .map(|x| buffer[(x, y)].symbol().to_string())
        .collect::<String>()
        .trim_end()
        .to_string()
}

/// The help rows (below the tab bar) that name the screen `title`.
fn help_rows_of(terminal: &Terminal<TestBackend>, title: &str) -> Vec<String> {
    (2..terminal.backend().buffer().area.height)
        .map(|y| row(terminal, y))
        .map(|r| r.trim_start_matches([' ', '│']).to_string())
        .filter(|r| r.contains(title))
        .collect()
}

/// The screen shown, by what its list says.
fn shown(terminal: &Terminal<TestBackend>) -> &'static str {
    let all = text(terminal);
    if all.contains("shown-second") {
        "second"
    } else if all.contains("shown-third") {
        "third"
    } else {
        "items"
    }
}

#[test]
fn the_default_keys_jump_and_show_in_the_tab_bar() {
    let (_dir, mut prepared, mut events, _log) = setup_with(&config(""), jumps_log());
    let terminal = run(&mut prepared, &mut events, &[]);
    assert_eq!(shown(&terminal), "items");
    assert!(row(&terminal, 1).contains("s:Second") && row(&terminal, 1).contains("u:Third"));
    assert_eq!(shown(&run(&mut prepared, &mut events, &["s"])), "second");
    assert_eq!(shown(&run(&mut prepared, &mut events, &["i"])), "items");
    assert_eq!(shown(&run(&mut prepared, &mut events, &["u"])), "third");
}

#[test]
fn a_rebound_key_jumps_and_the_old_one_is_inert() {
    let (_dir, mut prepared, mut events, _log) =
        setup_with(&config("S='goto_second'\n"), jumps_log());
    let terminal = run(&mut prepared, &mut events, &[]);
    let tabs = row(&terminal, 1);
    assert!(
        tabs.contains("S:Second") && !tabs.contains("s:Second"),
        "{tabs}"
    );
    assert!(
        tabs.contains("u:Third"),
        "the other jump keeps its key: {tabs}"
    );
    assert_eq!(shown(&run(&mut prepared, &mut events, &["s"])), "items");
    assert_eq!(shown(&run(&mut prepared, &mut events, &["S"])), "second");
    assert_eq!(shown(&run(&mut prepared, &mut events, &["i"])), "items");
    assert_eq!(shown(&run(&mut prepared, &mut events, &["u"])), "third");
    // The help lists the key that works.
    let terminal = run(&mut prepared, &mut events, &["i", "?"]);
    let rows = help_rows_of(&terminal, "Second");
    assert!(
        rows.iter().any(|r| r.trim_start().starts_with('S')),
        "the help row for Second shows S: {rows:?}"
    );
    assert!(
        !rows.iter().any(|r| r.trim_start().starts_with("s ")),
        "no help row for the old key: {rows:?}"
    );
}

#[test]
fn noop_removes_the_jump_its_tab_key_and_help_row() {
    let (_dir, mut prepared, mut events, _log) = setup_with(&config("s='noop'\n"), jumps_log());
    let terminal = run(&mut prepared, &mut events, &[]);
    let tabs = row(&terminal, 1);
    assert!(
        tabs.contains("Second") && !tabs.contains("s:Second"),
        "{tabs}"
    );
    assert!(tabs.contains("u:Third"), "{tabs}");
    assert_eq!(shown(&run(&mut prepared, &mut events, &["s"])), "items");
    assert_eq!(shown(&run(&mut prepared, &mut events, &["u"])), "third");
    let terminal = run(&mut prepared, &mut events, &["i", "?"]);
    assert!(
        help_rows_of(&terminal, "Second").is_empty(),
        "{:?}",
        help_rows_of(&terminal, "Second")
    );
    assert!(!help_rows_of(&terminal, "Third").is_empty());
}

#[test]
fn an_unrelated_table_changes_nothing() {
    let (_d1, mut base, mut e1, _l1) = setup_with(&config(""), jumps_log());
    let want = run(&mut base, &mut e1, &[]).backend().buffer().clone();
    for entries in ["Z='noop'\n", "Q2='noop'\n"] {
        let (_d2, mut prepared, mut e2, _l2) = setup_with(&config(entries), jumps_log());
        let got = run(&mut prepared, &mut e2, &[]).backend().buffer().clone();
        assert_eq!(got, want, "{entries:?}");
    }
}

#[test]
fn a_screen_with_no_declared_key_has_a_jump_the_table_can_bind() {
    // The fixture's `graph` screen declares no key; with `graph` set the plugin id is `git`.
    let git_config = |entries: &str| format!("[ui]\nlang='en'\n[keys.git]\n{entries}");
    let log = || Log {
        graph: true,
        jumps: true,
        ..Log::default()
    };
    // Without the entry `Z` does nothing: the second screen stays.
    let (_d1, mut prepared, mut events, _l1) = setup_with(&git_config(""), log());
    run(&mut prepared, &mut events, &[]);
    let terminal = run(&mut prepared, &mut events, &["s", "Z"]);
    assert_eq!(shown(&terminal), "second", "no key by default");
    assert!(
        !row(&terminal, 1).contains("Z:"),
        "a keyless jump shows no key: {}",
        row(&terminal, 1)
    );
    // With `goto_graph` it leaves the second screen for the graph, and the tab bar shows it.
    let (_d2, mut prepared, mut events, _l2) = setup_with(&git_config("Z='goto_graph'\n"), log());
    let terminal = run(&mut prepared, &mut events, &[]);
    assert!(
        row(&terminal, 1).contains("Z:Graph"),
        "{}",
        row(&terminal, 1)
    );
    assert_eq!(shown(&run(&mut prepared, &mut events, &["s"])), "second");
    assert_ne!(shown(&run(&mut prepared, &mut events, &["Z"])), "second");
}

#[test]
fn a_refused_key_keeps_the_default_jump() {
    // `j` moves the cursor: the host's key cannot be a jump.
    let (_dir, mut prepared, mut events, _log) =
        setup_with(&config("j='goto_second'\n"), jumps_log());
    assert_eq!(shown(&run(&mut prepared, &mut events, &["j"])), "items");
    assert_eq!(shown(&run(&mut prepared, &mut events, &["s"])), "second");
}
