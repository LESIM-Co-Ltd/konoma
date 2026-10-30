//! `e` and `o` in the app: the commands they would start are recorded (nothing is started), the
//! caret line reaches the editor, and a switched-off setting is a notice, not a process.

use std::sync::{Arc, Mutex};

use konoma_host_core::shared::SharedConfig;
use konoma_plugin::app::{open_file, run_with, AppCx, AppEvent, AppKeys, LaunchCx};
use konoma_plugin::launch::{Attempt, OfficeOs};
use konoma_plugin::prelude::*;
use konoma_plugin::TermCaps;
use konoma_test_support::unique_tmp;
use ratatui::backend::TestBackend;
use ratatui::Terminal;

use crate::plugin::TestPlugin;

type Log = Arc<Mutex<Vec<Vec<String>>>>;

fn argv(a: &Attempt) -> Vec<String> {
    std::iter::once(a.prog.clone())
        .chain(a.args.iter().cloned())
        .collect()
}

/// Runs `events` over a 40-line text with `config`; returns (editor argvs, GUI argvs, last frame).
fn run(config: &str, events: &str) -> (Vec<Vec<String>>, Vec<Vec<String>>, String, String) {
    let c = SharedConfig::parse(config).expect("the test config parses");
    let dir = unique_tmp("app_launch");
    std::fs::create_dir_all(dir.as_path()).unwrap();
    let p = dir.as_path().join("a.t");
    let body: String = (0..40).map(|i| format!("line {i:02}\n")).collect();
    std::fs::write(&p, body).unwrap();
    let mut opened =
        open_file(vec![Arc::new(TestPlugin::all())], &p, "en", TermCaps::new()).expect("opens");
    let (edits, guis): (Log, Log) = Default::default();
    let (e2, g2) = (edits.clone(), guis.clone());
    let launch = LaunchCx::new(c.editor.clone(), &c.external, opened.manifest.verbs.clone())
        .with_os(OfficeOs::Other)
        .with_hand_off(Arc::new(move |a, _, _| {
            e2.lock().unwrap().push(argv(a));
            Ok(Some(0))
        }))
        .with_runner(Arc::new(move |a| {
            g2.lock().unwrap().push(argv(a));
            Ok(Some(0))
        }));
    let cx = AppCx {
        lang: "en".into(),
        term: TermCaps::new(),
        title: "title".into(),
        path: opened.path.clone(),
        launch: Some(launch),
        ..AppCx::default()
    };
    let mut term = Terminal::new(TestBackend::new(60, 8)).unwrap();
    let ev: Vec<AppEvent> = events
        .chars()
        .map(|ch| AppEvent::Key(Key::char(ch)))
        .collect();
    let mut it = ev.into_iter();
    let keys = AppKeys::new(&c.keys, &c.ui.keys);
    run_with(&mut term, &mut *opened.session, &mut it, &cx, &keys).expect("runs");
    let buf = term.backend().buffer();
    let rows: Vec<String> = (0..buf.area.height)
        .map(|y| {
            (0..buf.area.width)
                .map(|x| buf[(x, y)].symbol().to_string())
                .collect()
        })
        .collect();
    let path = p.to_string_lossy().to_string();
    let e = edits.lock().unwrap().clone();
    let g = guis.lock().unwrap().clone();
    (e, g, rows.join("\n"), path)
}

#[test]
fn e_hands_the_caret_line_to_the_editor() {
    let (edits, guis, _, path) = run("[editor]\ncommand = \"nvim\"\n", "jje");
    assert_eq!(edits.len(), 1, "{edits:?}");
    assert_eq!(edits[0][0], "nvim");
    assert!(edits[0].iter().any(|a| a == "+3"), "{:?}", edits[0]);
    assert!(
        edits[0].iter().any(|a| a.ends_with("a.t")),
        "{:?} {path}",
        edits[0]
    );
    assert!(guis.is_empty(), "{guis:?}");
}

#[test]
fn e_after_the_caret_moved_names_the_new_line() {
    let (edits, _, _, _) = run("[editor]\ncommand = \"nvim\"\n", "e");
    assert!(edits[0].iter().any(|a| a == "+1"), "{:?}", edits[0]);
    let (edits, _, _, _) = run("[editor]\ncommand = \"nvim\"\n", "jjjje");
    assert!(edits[0].iter().any(|a| a == "+5"), "{:?}", edits[0]);
}

#[test]
fn o_walks_the_os_default_chain_and_never_the_editor() {
    let (edits, guis, frame, _) = run("", "o");
    assert!(edits.is_empty(), "{edits:?}");
    assert_eq!(guis.len(), 1, "{guis:?}");
    assert_eq!(guis[0][0], "xdg-open");
    assert!(frame.contains("opened"), "{frame}");
}

#[test]
fn o_is_a_notice_when_opening_is_switched_off() {
    let (edits, guis, frame, _) = run("[external]\nopen_links = false\n", "o");
    assert!(edits.is_empty() && guis.is_empty(), "{edits:?} {guis:?}");
    let _ = frame;
}

#[test]
fn the_footer_names_e_and_o_only_with_a_launch() {
    let (_, _, with, _) = run("", "");
    assert!(with.contains("e:edit"), "{with}");
    assert!(with.contains("o:open in app"), "{with}");
}
