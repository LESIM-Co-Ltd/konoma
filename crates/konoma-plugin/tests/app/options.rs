//! The `,` menu of the app: `p` cycles the way the title writes the path (relative, `~/`, full)
//! and comes round again; `w` turns the wrapping of the text off and on. Each item is seen in
//! the drawn frame, so an item that is not kept in the app's display state turns this red.

use std::sync::Arc;

use konoma_host_core::paths::TestHome;
use konoma_plugin::app::{open_file, run_with, AppCx, AppEvent, AppKeys};
use konoma_plugin::prelude::*;
use konoma_plugin::TermCaps;
use konoma_test_support::unique_tmp;
use ratatui::backend::TestBackend;
use ratatui::Terminal;

use crate::plugin::TestPlugin;

fn key(c: char) -> AppEvent {
    AppEvent::Key(Key::char(c))
}

fn chars(s: &str) -> Vec<AppEvent> {
    s.chars().filter(|c| *c != ' ').map(key).collect()
}

/// The rows of the frame after the keys; `a.t` sits in `<root>/proj/sub`, the app opened in
/// `<root>/proj`, and HOME is `<root>` (this thread's override, the environment is untouched).
fn rows_after(keys: &str, body: &str) -> Vec<String> {
    let dir = unique_tmp("app_options");
    let root = std::fs::canonicalize(dir.as_path()).unwrap_or_else(|_| {
        std::fs::create_dir_all(dir.as_path()).unwrap();
        std::fs::canonicalize(dir.as_path()).unwrap()
    });
    let open_dir = root.join("proj");
    std::fs::create_dir_all(open_dir.join("sub")).unwrap();
    let p = open_dir.join("sub/a.t");
    std::fs::write(&p, body).unwrap();
    let _home = TestHome::set(&root);
    let mut opened =
        open_file(vec![Arc::new(TestPlugin::all())], &p, "en", TermCaps::new()).expect("opens");
    let mut term = Terminal::new(TestBackend::new(240, 12)).unwrap();
    let cx = AppCx {
        lang: "en".into(),
        term: TermCaps::new(),
        title: "title".into(),
        path: opened.path.clone(),
        open_dir: Some(open_dir),
        ..AppCx::default()
    };
    let mut it = chars(keys).into_iter();
    run_with(
        &mut term,
        &mut *opened.session,
        &mut it,
        &cx,
        &AppKeys::defaults(),
    )
    .expect("runs");
    assert_eq!(it.count(), 0, "every key was read: {keys}");
    let buf = term.backend().buffer();
    (0..buf.area.height)
        .map(|y| {
            (0..buf.area.width)
                .map(|x| buf[(x, y)].symbol())
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect()
}

fn title(rows: &[String]) -> &str {
    rows.iter().find(|r| r.contains("a.t")).expect("a title")
}

#[test]
fn p_cycles_the_title_through_the_three_styles_and_comes_round() {
    let body = "x\n";
    let start = rows_after("", body);
    let home = rows_after(",p", body);
    let full = rows_after(",p,p", body);
    let again = rows_after(",p,p,p", body);
    let (t0, t1, t2, t3) = (title(&start), title(&home), title(&full), title(&again));
    assert!(t0.contains("proj/sub/a.t") && !t0.contains('~'), "{t0}");
    assert!(t1.contains("~/proj/sub/a.t"), "{t1}");
    assert!(
        t2.contains("/proj/sub/a.t") && !t2.contains('~') && t2 != t0,
        "{t2}"
    );
    assert!(t0 != t1 && t1 != t2 && t0 != t2);
    assert_eq!(t3, t0, "a whole cycle comes round");
    assert_eq!(again, start, "and the frame with it");
}

#[test]
fn w_turns_the_wrapping_off_and_on() {
    // One line far wider than the screen: wrapped it fills several rows, cut it fills one.
    let body = format!("{}\n", "word ".repeat(100));
    let filled = |rows: &[String]| rows.iter().filter(|r| r.contains("word")).count();
    let start = rows_after("", &body);
    let cut = rows_after(",w", &body);
    let back = rows_after(",w,w", &body);
    assert!(filled(&start) > 1, "wrapped at the start: {start:?}");
    assert_eq!(filled(&cut), 1, "cut after w: {cut:?}");
    assert_eq!(back, start, "w twice comes round");
}
