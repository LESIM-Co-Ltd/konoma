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
    // The key answers as konoma's does, with the shared notice (the screen is narrow, so the
    // front of the line is compared; the parity test in konoma compares the whole of it); the
    // footer does not name a key that does not act.
    let notice = konoma_host_core::help_msg::CoreMsg::ExternalOpenLinksDisabled
        .tr(konoma_host_core::lang::Lang::En);
    assert!(frame.contains(&notice[..40]), "{frame}");
    assert!(
        !frame.contains("opened") && !frame.contains("o:open in app"),
        "{frame}"
    );
}

#[test]
fn o_without_a_launch_answers_nothing() {
    // No `LaunchCx`: the program has neither key, so there is no notice either.
    let c = SharedConfig::parse("").expect("parses");
    let keys = AppKeys::new(&c.keys, &c.ui.keys);
    let key = Key::char('o');
    assert!(!keys.acts_on(&key, false, true));
    assert!(!keys.table_acts_on(&key, false, Default::default()));
    assert!(!keys.canvas_acts_on(&key, false));
}

#[test]
fn the_footer_names_e_and_o_only_with_a_launch() {
    let (_, _, with, _) = run("", "");
    assert!(with.contains("e:edit"), "{with}");
    assert!(with.contains("o:open in app"), "{with}");
}

/// A converter that declares `e` as the GUI program `docx-edit`, and shows its file as a document
/// until it is asked a second time, when it hands the text over as Markdown (a `Reopen`).
struct Converter;

impl Plugin for Converter {
    fn manifest(&self) -> Manifest {
        use konoma_plugin::manifest::{LaunchMode, Launcher, Verb, VerbDecl};
        Manifest::new("converter", "0.0.1")
            .bind(Bind::files(["*.conv"]))
            .shows(ViewKind::Document)
            .converts()
            .verb(
                VerbDecl::new(Verb::Edit)
                    .launcher(Launcher::Command(vec!["docx-edit".into()]))
                    .mode(LaunchMode::Gui),
            )
    }
    fn open(&self, _input: Input, _cx: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        Ok(Box::new(ConvSession { asked: 0 }))
    }
}

struct ConvSession {
    asked: usize,
}

fn doc(markdown: &str) -> konoma_plugin::view::DocumentView {
    konoma_plugin::view::DocumentView {
        markdown: markdown.to_string(),
        origin: konoma_plugin::input::Origin::File(std::path::PathBuf::from("x.md")),
        resources: Default::default(),
        options: konoma_plugin::view::DocOptions::default(),
        title: None,
        truncated: false,
    }
}

impl Session for ConvSession {
    fn view(&mut self, _req: &ViewReq, _cx: &ViewCx) -> Result<View, Failure> {
        self.asked += 1;
        if self.asked == 1 {
            return Ok(View::Document(doc("old\n")));
        }
        Ok(View::Reopen(konoma_plugin::view::Reopen {
            content_type: "markdown".into(),
            text: "new\n".into(),
            resources: Default::default(),
            origin: konoma_plugin::input::Origin::Converted {
                from: std::path::PathBuf::from("a.conv"),
            },
            truncated: false,
        }))
    }
}

/// The destination of the `Reopen`: a plugin that declares its own `e` (`dest-edit`), which must
/// not be used for the file the converter took over.
struct Dest;

impl Plugin for Dest {
    fn manifest(&self) -> Manifest {
        use konoma_plugin::manifest::{LaunchMode, Launcher, Verb, VerbDecl};
        Manifest::new("dest", "0.0.1")
            .bind(Bind::content("markdown"))
            .shows(ViewKind::Document)
            .verb(
                VerbDecl::new(Verb::Edit)
                    .launcher(Launcher::Command(vec!["dest-edit".into()]))
                    .mode(LaunchMode::Gui),
            )
    }
    fn open(&self, input: Input, _cx: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        let Input::Content { text, .. } = input else {
            return Err(Failure::unsupported(t!(en: "content only")));
        };
        Ok(Box::new(DestSession { text }))
    }
}

struct DestSession {
    text: String,
}

impl Session for DestSession {
    fn view(&mut self, _req: &ViewReq, _cx: &ViewCx) -> Result<View, Failure> {
        Ok(View::Document(doc(&self.text)))
    }
}

/// After a `Reopen`, `e` still goes where the plugin that took the file over says (the file is a
/// docx), not where the plugin showing its text now says.
#[test]
fn e_after_a_reopen_follows_the_declaration_of_the_plugin_that_took_the_file() {
    let c = SharedConfig::parse("").expect("config");
    let dir = unique_tmp("app_launch_reopen");
    std::fs::create_dir_all(dir.as_path()).unwrap();
    let p = dir.as_path().join("a.conv");
    std::fs::write(&p, "x").unwrap();
    let mut opened =
        open_file(vec![Arc::new(Converter)], &p, "en", TermCaps::new()).expect("opens");
    let guis: Log = Default::default();
    let g2 = guis.clone();
    let launch = LaunchCx::new(c.editor.clone(), &c.external, opened.manifest.verbs.clone())
        .with_os(OfficeOs::Other)
        .with_runner(Arc::new(move |a| {
            g2.lock().unwrap().push(argv(a));
            Ok(Some(0))
        }));
    let source = konoma_plugin::app::ReopenSource::new(|ct| {
        assert_eq!(ct, "markdown");
        Some(konoma_plugin::reopen::ContentTarget {
            plugin: Arc::new(Dest),
            settings: Settings::empty().clone(),
        })
    });
    let cx = AppCx {
        lang: "en".into(),
        term: TermCaps::new(),
        path: opened.path.clone(),
        shows: opened.shows.clone(),
        converts: opened.manifest.converts,
        reopen: Some(source),
        launch: Some(launch),
        ..AppCx::default()
    };
    let mut term = Terminal::new(TestBackend::new(60, 8)).unwrap();
    // The resize asks the converter again (it answers `Reopen`); `e` comes after it.
    let events = vec![AppEvent::Resize, AppEvent::Key(Key::char('e'))];
    let mut it = events.into_iter();
    run_with(
        &mut term,
        &mut *opened.session,
        &mut it,
        &cx,
        &AppKeys::defaults(),
    )
    .expect("runs");
    let buf = term.backend().buffer();
    let frame: String = (0..buf.area.height)
        .flat_map(|y| (0..buf.area.width).map(move |x| (x, y)))
        .map(|(x, y)| buf[(x, y)].symbol().to_string())
        .collect();
    assert!(frame.contains("new"), "the destination is shown: {frame}");
    let ran = guis.lock().unwrap().clone();
    assert_eq!(ran.len(), 1, "{ran:?}");
    assert_eq!(ran[0][0], "docx-edit", "{ran:?}");
}
