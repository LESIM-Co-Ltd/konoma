//! A `Reopen` in the app loop: the destination is looked up by content type, its session
//! replaces the converting one, and nothing of the old document stays on the new screen.

use std::sync::{Arc, Mutex};

use konoma_plugin::app::{open_file, run_with, AppCx, AppEvent, AppKeys, ReopenSource};
use konoma_plugin::input::Origin;
use konoma_plugin::prelude::*;
use konoma_plugin::reopen::ContentTarget;
use konoma_plugin::view::{DocOptions, DocumentView, Reopen};
use konoma_plugin::TermCaps;
use konoma_test_support::unique_tmp;
use ratatui::backend::TestBackend;
use ratatui::Terminal;

fn long(prefix: &str) -> String {
    (0..60).map(|i| format!("{prefix} {i:02}\n\n")).collect()
}

fn doc(markdown: String) -> DocumentView {
    DocumentView {
        markdown,
        origin: Origin::File(std::path::PathBuf::from("x.md")),
        resources: Default::default(),
        options: DocOptions::default(),
        title: None,
        truncated: false,
    }
}

/// Shows a long document of "old" lines the first time it is asked, and from then on answers a
/// `Reopen` of a long text of "new" lines as `content_type`.
struct Converter {
    content_type: &'static str,
}

impl Plugin for Converter {
    fn manifest(&self) -> Manifest {
        Manifest::new("converter", "0.0.1")
            .bind(Bind::files(["*.conv"]))
            .shows(ViewKind::Document)
    }
    fn open(&self, _input: Input, _cx: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        Ok(Box::new(ConvSession {
            asked: 0,
            content_type: self.content_type,
        }))
    }
}

struct ConvSession {
    asked: usize,
    content_type: &'static str,
}

impl Session for ConvSession {
    fn view(&mut self, _req: &ViewReq, _cx: &ViewCx) -> Result<View, Failure> {
        self.asked += 1;
        if self.asked == 1 {
            return Ok(View::Document(doc(long("old"))));
        }
        Ok(View::Reopen(Reopen {
            content_type: self.content_type.to_string(),
            text: long("new"),
            resources: Default::default(),
            origin: Origin::Converted {
                from: std::path::PathBuf::from("a.conv"),
            },
            truncated: false,
        }))
    }
}

/// Takes `markdown` content and shows it as it is.
struct Target;

impl Plugin for Target {
    fn manifest(&self) -> Manifest {
        Manifest::new("target", "0.0.1")
            .bind(Bind::content("markdown"))
            .shows(ViewKind::Document)
    }
    fn open(&self, input: Input, _cx: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        let Input::Content { text, .. } = input else {
            return Err(Failure::unsupported(t!(en: "content only")));
        };
        Ok(Box::new(TargetSession { text }))
    }
}

struct TargetSession {
    text: String,
}

impl Session for TargetSession {
    fn view(&mut self, _req: &ViewReq, _cx: &ViewCx) -> Result<View, Failure> {
        Ok(View::Document(doc(self.text.clone())))
    }
}

/// Runs the app on a converted file with `events`; `lookups` collects the content types asked.
fn run(
    content_type: &'static str,
    source: Option<ReopenSource>,
    events: Vec<AppEvent>,
) -> Vec<String> {
    let dir = unique_tmp("app_reopen");
    std::fs::create_dir_all(dir.as_path()).unwrap();
    let p = dir.as_path().join("a.conv");
    std::fs::write(&p, "x").unwrap();
    let mut opened = open_file(
        vec![Arc::new(Converter { content_type })],
        &p,
        "en",
        TermCaps::new(),
    )
    .expect("opens");
    let mut term = Terminal::new(TestBackend::new(60, 10)).unwrap();
    let cx = AppCx {
        lang: "en".into(),
        term: TermCaps::new(),
        title: String::new(),
        shows: opened.shows.clone(),
        reopen: source,
        ..AppCx::default()
    };
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

fn to_target(asked: Arc<Mutex<Vec<String>>>) -> ReopenSource {
    ReopenSource::new(move |ct| {
        asked.lock().unwrap().push(ct.to_string());
        Some(ContentTarget {
            plugin: Arc::new(Target),
            settings: Settings::empty().clone(),
        })
    })
}

fn key(c: char) -> AppEvent {
    AppEvent::Key(Key::char(c))
}

#[test]
fn a_reopen_shows_the_destination_document_from_its_top_not_where_the_old_one_was_scrolled() {
    let asked = Arc::new(Mutex::new(Vec::new()));
    // Scroll the old document to its end, then a resize asks the session again: it answers `Reopen`.
    let rows = run(
        "markdown",
        Some(to_target(asked.clone())),
        vec![key('G'), AppEvent::Resize],
    );
    assert_eq!(asked.lock().unwrap().as_slice(), ["markdown"]);
    assert!(rows[1].contains("new 00"), "{rows:?}");
    assert!(rows.iter().all(|r| !r.contains("old ")), "{rows:?}");
}

#[test]
fn after_a_reopen_the_keys_go_to_the_destination_session() {
    let asked = Arc::new(Mutex::new(Vec::new()));
    // The converter is asked once more per view; were it still asked, the second `Resize` would
    // reopen again and the scroll of `j` would be lost.
    let rows = run(
        "markdown",
        Some(to_target(asked.clone())),
        vec![AppEvent::Resize, key('j'), key('j'), AppEvent::Resize],
    );
    assert_eq!(
        asked.lock().unwrap().len(),
        1,
        "{:?}",
        asked.lock().unwrap()
    );
    assert!(rows[1].contains("new 01"), "{rows:?}");
}

#[test]
fn a_reopen_with_no_destination_says_no_plugin_is_available() {
    let rows = run("markdown", None, vec![AppEvent::Resize]);
    assert_eq!(rows[0], "no plugin is available to show this document");
}

#[test]
fn a_reopen_as_a_content_type_the_app_cannot_show_says_no_plugin_is_available() {
    let asked = Arc::new(Mutex::new(Vec::new()));
    let rows = run(
        "html",
        Some(to_target(asked.clone())),
        vec![AppEvent::Resize],
    );
    assert_eq!(rows[0], "no plugin is available to show this document");
}
