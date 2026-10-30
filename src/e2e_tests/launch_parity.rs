//! `e` and `o` in the konoma host and in the standalone app (crates/konoma-plugin, feature `app`):
//! the same plugin, the same settings, the same verbs. Compared: what would have been started
//! (argv of the editor, the terminal chain and the GUI chain, recorded through the seams; no
//! process is ever started), the notice, and the `e` / `o` hints of the footer.
//!
//! The plugin (`Launchy`) shows text, a table, a document or a picture and declares the verbs a
//! case asks for; the `Hexdump` test plugin declares them for a terminal chain; a Word file is
//! shown by the real office and Markdown plugins, which `Reopen` it as Markdown.

use super::image_parity::WhenIdle;
use super::table_parity::{hints, rows_of, APP_H, HOST_H, W};
use super::*;
use konoma_plugin::app::{
    key_from_crossterm, prepare_bundled, run_with_pictures, AppCx, AppEvent, Display,
};
use konoma_plugin::launch::{Attempt, OfficeOs};
use konoma_plugin::prelude::*;
use konoma_plugin::view::{Grid, TableView};
use konoma_plugin::{LaunchMode, Launcher, TermCaps, Verb, VerbDecl};
use konoma_test_support::TermScreen;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Shape {
    Text,
    Table,
    Document,
    Image,
}

/// Who shows the file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Engine {
    /// The `Launchy` plugin, in the shape of the case.
    Launchy,
    /// The `Hexdump` test plugin (text) declaring the verbs.
    Hexdump,
    /// A Word file shown by the real office plugin and `Reopen`ed as Markdown.
    Docx,
}

#[derive(Clone)]
struct Case {
    name: &'static str,
    engine: Engine,
    shape: Shape,
    verbs: Vec<VerbDecl>,
    toml: &'static str,
    keys: &'static str,
    /// A flag the editor must have been started with (in the app's record of it).
    editor_has: Option<&'static str>,
}

impl Case {
    fn file(&self) -> &'static str {
        match (self.engine, self.shape) {
            (Engine::Docx, _) => "d.docx",
            (Engine::Hexdump, _) => "h.bin",
            (_, Shape::Image) => "i.lnc",
            _ => "a.lnc",
        }
    }
}

fn cmd(prog: &str, mode: LaunchMode, verb: Verb) -> VerbDecl {
    VerbDecl::new(verb)
        .launcher(Launcher::Command(vec![prog.into(), "{path}".into()]))
        .mode(mode)
}

/// Like [`cmd`], the line the caret is on given as an argument (`{line}` is filled in for a
/// terminal chain), so the argv shows which line `e` was pressed on.
fn cmd_at(prog: &str, mode: LaunchMode, verb: Verb) -> VerbDecl {
    VerbDecl::new(verb)
        .launcher(Launcher::Command(vec![
            prog.into(),
            "--at={line}".into(),
            "{path}".into(),
        ]))
        .mode(mode)
}

struct Launchy {
    id: String,
    shape: Shape,
    verbs: Vec<VerbDecl>,
}

struct OneCell;
impl Grid for OneCell {
    fn nrows(&self) -> usize {
        1
    }
    fn ncols(&self) -> usize {
        1
    }
    fn header(&self, _: usize) -> Option<String> {
        Some("col".into())
    }
    fn cell(&self, _: usize, _: usize) -> String {
        "cell".into()
    }
}

struct Sess(Shape, std::path::PathBuf);

impl Session for Sess {
    fn view(&mut self, _: &ViewReq, _: &ViewCx) -> Result<View, Failure> {
        Ok(match self.0 {
            Shape::Image => View::Image(konoma_plugin::view::ImageView::new(
                konoma_plugin::view::ImageData::File(self.1.clone()),
            )),
            Shape::Text => {
                // The lines are the file's (`a.lnc` has the same 30), so `e` can open at the caret.
                let mut v = View::text((0..30).map(|i| format!("line {i:02}")));
                if let View::Text(t) = &mut v {
                    t.file_lines = true;
                }
                v
            }
            Shape::Table => View::Table(TableView {
                grid: Arc::new(OneCell),
                title: None,
                rainbow: false,
                addressed: false,
            }),
            Shape::Document => View::Document(konoma_plugin::view::DocumentView {
                markdown: "# Title\n\nbody".into(),
                origin: konoma_plugin::Origin::File("a.lnc".into()),
                resources: Default::default(),
                options: Default::default(),
                title: None,
                truncated: false,
            }),
        })
    }
}

impl Plugin for Launchy {
    fn manifest(&self) -> Manifest {
        let kind = match self.shape {
            Shape::Text => ViewKind::Text,
            Shape::Table => ViewKind::Table,
            Shape::Document => ViewKind::Document,
            Shape::Image => ViewKind::Image,
        };
        let mut m = Manifest::new(self.id.as_str(), "0.1.0")
            .bind(Bind::files(["*.lnc"]))
            .shows(kind);
        for v in &self.verbs {
            m = m.verb(v.clone());
        }
        m
    }
    fn open(&self, input: Input, _: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        let path = match input {
            Input::File { path, .. } => path,
            _ => std::path::PathBuf::new(),
        };
        Ok(Box::new(Sess(self.shape, path)))
    }
}

static SEQ: AtomicUsize = AtomicUsize::new(0);

/// What was started, in order, as `kind:argv` (the editor as `edit:<line>`).
type Started = Arc<Mutex<Vec<String>>>;

struct Seen {
    started: Vec<String>,
    /// The `e` / `o` tokens of the footer.
    hints: Vec<String>,
    /// The notice, if one was shown.
    notice: Option<String>,
    /// The editor's argv as the app started it (the host records only the line).
    editor_argv: Vec<String>,
}

/// The editor of the cases: the line reaches it as `+<n>`.
const EDITOR: &str = "nvim";

/// The line an `nvim` argv was asked to open at.
fn editor_line(a: &Attempt) -> Option<usize> {
    a.args
        .iter()
        .find_map(|x| x.strip_prefix('+')?.parse().ok())
}

/// A `Hexdump` plugin declaring `verbs`.
fn hexdump(verbs: &[VerbDecl]) -> Arc<dyn Plugin> {
    Arc::new(crate::plugins::test_plugins::Hexdump(
        crate::plugins::test_plugins::Variant {
            verbs: verbs.to_vec(),
            ..Default::default()
        },
    ))
}

/// The plugins the app is given for the case.
fn app_plugins(c: &Case, id: String) -> Vec<Arc<dyn Plugin>> {
    match c.engine {
        Engine::Launchy => vec![Arc::new(Launchy {
            id,
            shape: c.shape,
            verbs: c.verbs.clone(),
        })],
        Engine::Hexdump => vec![hexdump(&c.verbs)],
        Engine::Docx => vec![
            Arc::new(konoma_office::DocumentPlugin),
            Arc::new(konoma_markdown::MarkdownPlugin),
        ],
    }
}

fn argv(a: &Attempt) -> String {
    std::iter::once(a.prog.clone())
        .chain(a.args.iter().cloned())
        .collect::<Vec<_>>()
        .join(" ")
}

fn eo(hs: Vec<String>) -> Vec<String> {
    hs.into_iter()
        .filter(|h| h.starts_with("e:") || h.starts_with("o:"))
        .collect()
}

fn config_text(c: &Case) -> String {
    format!("[ui]\nlang = \"en\"\n{}", c.toml)
}

fn host(root: &std::path::Path, c: &Case) -> Seen {
    let id = format!("lnch-{}", SEQ.fetch_add(1, Ordering::SeqCst));
    let mut cfg = toml::from_str::<crate::config::Config>(&config_text(c)).expect("a config");
    if c.engine != Engine::Docx {
        let plugin = app_plugins(c, id.clone()).remove(0);
        let marker = crate::plugins::test_plugins::register(&id, plugin);
        let mut t = toml::Table::new();
        t.insert("enabled".into(), toml::Value::Boolean(false));
        cfg.plugins.insert(marker, t);
    }
    let mut s = Sim::with_config_sized(root, cfg, W, HOST_H);
    super::image_parity::open_nested(&mut s, c.file());
    let started: Started = Default::default();
    let st = started.clone();
    s.app.set_office_runner(Arc::new(move |a: &Attempt| {
        st.lock().unwrap().push(format!("gui:{}", argv(a)));
        Ok(Some(0))
    }));
    s.draw();
    let mut out = Vec::new();
    for ch in c.keys.chars() {
        s.key(ch);
        out.append(&mut started.lock().unwrap());
        if let Some((_, line)) = s.app.take_pending_edit() {
            out.push(format!("edit:{line:?}"));
        }
        if let Some((chain, _)) = s.app.take_pending_terminal() {
            out.push(format!(
                "term:{}",
                chain.iter().map(argv).collect::<Vec<_>>().join(" | ")
            ));
        }
        s.draw();
    }
    let notice = s.app.flash.clone();
    s.app.flash = None;
    s.term.clear().expect("clear");
    s.draw();
    let rows = rows_of(&s.term);
    Seen {
        started: out,
        hints: eo(hints(rows.last().expect("a footer"))),
        notice,
        editor_argv: Vec::new(),
    }
}

fn app(root: &std::path::Path, c: &Case) -> Seen {
    let id = format!("lnch-{}", SEQ.fetch_add(1, Ordering::SeqCst));
    let text = config_text(c);
    let prepared = prepare_bundled(
        app_plugins(c, id),
        Vec::new(),
        &root.join(c.file()),
        TermCaps::new(),
        (
            konoma_host_core::shared::SharedConfig::parse(&text).expect("a config"),
            None,
        ),
    )
    .map_err(|f| f.error)
    .expect("prepares");
    let konoma_plugin::app::Prepared {
        mut opened,
        keys,
        lang,
        reopen,
        acts,
        launch,
        ..
    } = prepared;
    let started: Started = Default::default();
    let editor_argv: Started = Default::default();
    let (s1, s2, s3) = (started.clone(), started.clone(), editor_argv.clone());
    let launch = launch
        .with_os(OfficeOs::current())
        .with_hand_off(Arc::new(move |a, _, _| {
            // The editor is recorded as the host records it, by the line it was asked to open at.
            if a.prog == EDITOR {
                s3.lock().unwrap().push(argv(a));
                s1.lock()
                    .unwrap()
                    .push(format!("edit:{:?}", editor_line(a)));
            } else {
                s1.lock().unwrap().push(format!("term:{}", argv(a)));
            }
            Ok(Some(0))
        }))
        .with_runner(Arc::new(move |a| {
            s2.lock().unwrap().push(format!("gui:{}", argv(a)));
            Ok(Some(0))
        }));
    let mut term = Terminal::new(TermScreen::new(W, APP_H)).unwrap();
    let cx = AppCx {
        lang: lang.to_string(),
        term: TermCaps::new(),
        title: opened.title.clone(),
        unavailable: opened.unavailable.clone(),
        plugin_id: opened.plugin_id.clone(),
        path: opened.path.clone(),
        hint: opened.hint.clone(),
        shows: opened.shows.clone(),
        reopen: Some(reopen),
        open_dir: Some(root.to_path_buf()),
        display: Display::default(),
        acts,
        launch: Some(launch),
        converts: opened.manifest.converts,
    };
    let evs = c
        .keys
        .chars()
        .map(|ch| {
            let (code, m) = super::table_parity::token(&ch.to_string());
            AppEvent::Key(key_from_crossterm(&KeyEvent::new(code, m)).expect("a key"))
        })
        .collect();
    let mut ev = WhenIdle {
        keys: evs,
        turns: 0,
    };
    run_with_pictures(&mut term, &mut *opened.session, &mut ev, &cx, &keys, None).unwrap();
    let buf = term.backend().1.clone();
    let mut shown = Terminal::new(ratatui::backend::TestBackend::new(
        buf.area.width,
        buf.area.height,
    ))
    .unwrap();
    shown.draw(|f| *f.buffer_mut() = buf.clone()).expect("draw");
    let rows = rows_of(&shown);
    let started = started.lock().unwrap().clone();
    let editor_argv = editor_argv.lock().unwrap().clone();
    Seen {
        started,
        hints: eo(hints(rows.last().expect("a footer"))),
        notice: Some(rows.join("\n")),
        editor_argv,
    }
}

fn cases() -> Vec<Case> {
    let none = Vec::new();
    let gui = vec![
        cmd("lc-edit", LaunchMode::Gui, Verb::Edit),
        cmd("lc-open", LaunchMode::Gui, Verb::Open),
    ];
    let term = vec![cmd("lc-term", LaunchMode::Terminal, Verb::Edit)];
    let term_at = vec![cmd_at("lc-term", LaunchMode::Terminal, Verb::Edit)];
    let editor = "[editor]\ncommand = \"nvim\"\n";
    let ext_lnc = "[editor]\ncommand = \"nvim\"\n[editor.ext]\nlnc = \"nvim --clean\"\n";
    let ext_docx = "[editor]\ncommand = \"nvim\"\n[editor.ext]\ndocx = \"nvim --clean\"\n";
    let ext_bin = "[editor]\ncommand = \"nvim\"\n[editor.ext]\nbin = \"nvim --clean\"\n";
    let off = "[external]\nopen_links = false\noffice_apps = false\n";
    let mut v = Vec::new();
    // The shapes of a view: the same plugin and the same verbs in each.
    for shape in [Shape::Text, Shape::Table, Shape::Document, Shape::Image] {
        let mk = |name, verbs: Vec<VerbDecl>, toml, keys, editor_has| Case {
            name,
            engine: Engine::Launchy,
            shape,
            verbs,
            toml,
            keys,
            editor_has,
        };
        v.push(mk("no verb", none.clone(), editor, "eo", None));
        v.push(mk("gui verbs", gui.clone(), editor, "eo", None));
        v.push(mk("terminal verb", term.clone(), editor, "e", None));
        v.push(mk(
            "terminal verb, caret moved",
            term_at.clone(),
            editor,
            "jje",
            None,
        ));
        v.push(mk("external off, o", gui.clone(), off, "o", None));
        v.push(mk("external off, e", gui.clone(), off, "e", None));
        v.push(mk(
            "open_links off only, o",
            none.clone(),
            "[external]\nopen_links = false\n",
            "o",
            None,
        ));
        v.push(mk(
            "editor ext explicit",
            none.clone(),
            ext_lnc,
            "e",
            Some("--clean"),
        ));
        v.push(mk("caret moved", none.clone(), editor, "jje", None));
        v.push(mk(
            "caret moved far",
            none.clone(),
            editor,
            "jjjjjjje",
            None,
        ));
    }
    // A hexdump (text) plugin with a terminal chain, a GUI chain and no verb.
    let hex = |name, verbs: Vec<VerbDecl>, toml, keys, editor_has| Case {
        name,
        engine: Engine::Hexdump,
        shape: Shape::Text,
        verbs,
        toml,
        keys,
        editor_has,
    };
    v.push(hex("hexdump terminal", term.clone(), editor, "e", None));
    v.push(hex(
        "hexdump terminal, caret moved",
        term_at.clone(),
        editor,
        "jjje",
        None,
    ));
    v.push(hex("hexdump gui", gui.clone(), editor, "eo", None));
    v.push(hex(
        "hexdump no verb, caret moved",
        none.clone(),
        editor,
        "jje",
        None,
    ));
    v.push(hex(
        "hexdump ext explicit",
        term.clone(),
        ext_bin,
        "e",
        Some("--clean"),
    ));
    v.push(hex("hexdump external off, o", term.clone(), off, "o", None));
    // A Word file the office plugin hands over as Markdown (a `Reopen`): the verbs stay the
    // Word file's, so the Office apps are started, never the Markdown plugin's editor.
    let docx = |name, toml, keys, editor_has| Case {
        name,
        engine: Engine::Docx,
        shape: Shape::Document,
        verbs: Vec::new(),
        toml,
        keys,
        editor_has,
    };
    v.push(docx("docx", editor, "eo", None));
    v.push(docx("docx, e", editor, "e", None));
    v.push(docx("docx, o", editor, "o", None));
    v.push(docx("docx caret moved", editor, "jje", None));
    v.push(docx(
        "docx office off, e",
        "[external]\noffice_apps = false\n",
        "e",
        None,
    ));
    v.push(docx(
        "docx office off, o",
        "[external]\noffice_apps = false\n",
        "o",
        None,
    ));
    v.push(docx(
        "docx open_links off only, o",
        "[external]\nopen_links = false\n",
        "o",
        None,
    ));
    v.push(docx("docx ext explicit", ext_docx, "e", Some("--clean")));
    v.push(docx(
        "docx ext explicit, caret moved",
        ext_docx,
        "jjje",
        Some("--clean"),
    ));
    v
}

#[test]
fn e_and_o_start_the_same_things_in_the_host_and_in_the_app() {
    let dir = sandbox("launch-parity");
    let body: String = (0..30).map(|i| format!("line {i:02}\n")).collect();
    std::fs::write(dir.join("a.lnc"), body).unwrap();
    write_png(&dir.join("i.png"), 8, 8);
    std::fs::rename(dir.join("i.png"), dir.join("i.lnc")).unwrap();
    std::fs::write(
        dir.join("h.bin"),
        (0u8..=255).cycle().take(400).collect::<Vec<_>>(),
    )
    .unwrap();
    super::word::build_docx(&dir.join("d.docx"), &docx_body());
    let root = canon(&dir);
    let (mut compared, mut noticed, mut lines) = (0, 0, std::collections::BTreeSet::new());
    for c in cases() {
        let (h, a) = (host(&root, &c), app(&root, &c));
        let who = format!("{} / {:?} / {:?}", c.name, c.engine, c.shape);
        let idle = Case {
            keys: "",
            ..c.clone()
        };
        let (h0, a0) = (host(&root, &idle), app(&root, &idle));
        assert_eq!(h0.hints, a0.hints, "hints: {who}");
        // The editor is recorded as `edit:<line>` on both sides, every GUI / terminal argv in full:
        // the host's record and the app's must be the same list.
        assert!(
            !h.started.is_empty() || h.notice.is_some(),
            "nothing happened: {who}"
        );
        assert_eq!(h.started, a.started, "started: {who}");
        for s in &h.started {
            if s.starts_with("edit:") {
                lines.insert(s.clone());
            }
        }
        if let Some(flag) = c.editor_has {
            assert!(
                a.editor_argv
                    .iter()
                    .any(|x| x.split(' ').any(|t| t == flag)),
                "the editor was not started with {flag}: {:?}: {who}",
                a.editor_argv
            );
        }
        // A notice the host shows (switched off, or a chain's outcome) is on the app's screen too.
        if let Some(n) = &h.notice {
            let screen = a.notice.as_deref().unwrap_or("");
            assert!(
                screen.contains(n.as_str()),
                "notice {n:?} missing in the app: {who}\n{screen}"
            );
            if n.contains("disabled") {
                noticed += 1;
            }
        }
        compared += 3;
    }
    // Both ends of the caret moves were seen: the line the editor was asked for is not always 1.
    assert!(lines.len() >= 3, "{lines:?}");
    assert!(noticed >= 1, "{noticed}");
    // (measured: 55 cases, 3 comparisons each = 165.)
    assert!(compared >= 160, "{compared}");
}

/// A Word file with enough paragraphs to scroll.
fn docx_body() -> String {
    let mut body = super::word::para("Quarterly notes");
    for i in 0..40 {
        body += &super::word::para(&format!("paragraph {i} of the converted document"));
    }
    body
}
