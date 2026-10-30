//! `e` and `o` in the konoma host and in the standalone app (crates/konoma-plugin, feature `app`):
//! the same plugin, the same settings, the same verbs. Compared: what would have been started
//! (argv of the editor, the terminal chain and the GUI chain, recorded through the seams; no
//! process is ever started), the notice, and the `e` / `o` hints of the footer.
//!
//! The plugin (`Launchy`) shows text or a table and declares the verbs a case asks for.

use super::image_parity::WhenIdle;
use super::table_parity::{hints, rows_of, APP_H, HOST_H, W};
use super::*;
use konoma_plugin::app::{
    key_from_crossterm, prepare_bundled, run_with_pictures, AppCx, AppEvent, Display, LaunchCx,
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
}

#[derive(Clone)]
struct Case {
    name: &'static str,
    shape: Shape,
    verbs: Vec<VerbDecl>,
    toml: &'static str,
    keys: &'static str,
}

fn cmd(prog: &str, mode: LaunchMode, verb: Verb) -> VerbDecl {
    VerbDecl::new(verb)
        .launcher(Launcher::Command(vec![prog.into(), "{path}".into()]))
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

struct Sess(Shape);

impl Session for Sess {
    fn view(&mut self, _: &ViewReq, _: &ViewCx) -> Result<View, Failure> {
        Ok(match self.0 {
            Shape::Text => View::text((0..30).map(|i| format!("line {i:02}"))),
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
        };
        let mut m = Manifest::new(self.id.as_str(), "0.1.0")
            .bind(Bind::files(["*.lnc"]))
            .shows(kind);
        for v in &self.verbs {
            m = m.verb(v.clone());
        }
        m
    }
    fn open(&self, _: Input, _: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        Ok(Box::new(Sess(self.shape)))
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
    let marker = crate::plugins::test_plugins::register(
        &id,
        Arc::new(Launchy {
            id: id.clone(),
            shape: c.shape,
            verbs: c.verbs.clone(),
        }),
    );
    let mut cfg = toml::from_str::<crate::config::Config>(&config_text(c)).expect("a config");
    let mut t = toml::Table::new();
    t.insert("enabled".into(), toml::Value::Boolean(false));
    cfg.plugins.insert(marker, t);
    let mut s = Sim::with_config_sized(root, cfg, W, HOST_H);
    super::image_parity::open_nested(&mut s, "a.lnc");
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
    }
}

fn app(root: &std::path::Path, c: &Case) -> Seen {
    let id = format!("lnch-{}", SEQ.fetch_add(1, Ordering::SeqCst));
    let plugin: Arc<dyn Plugin> = Arc::new(Launchy {
        id,
        shape: c.shape,
        verbs: c.verbs.clone(),
    });
    let text = config_text(c);
    let cfg = konoma_host_core::shared::SharedConfig::parse(&text).expect("a config");
    let prepared = prepare_bundled(
        vec![plugin],
        Vec::new(),
        &root.join("a.lnc"),
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
        ..
    } = prepared;
    let started: Started = Default::default();
    let (s1, s2) = (started.clone(), started.clone());
    let launch = LaunchCx::new(
        cfg.editor.clone(),
        &cfg.external,
        opened.manifest.verbs.clone(),
    )
    .with_os(OfficeOs::current())
    .with_hand_off(Arc::new(move |a, _, _| {
        s1.lock().unwrap().push(format!("hand:{}", argv(a)));
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
    Seen {
        started,
        hints: eo(hints(rows.last().expect("a footer"))),
        notice: Some(rows.join("\n")),
    }
}

fn cases() -> Vec<Case> {
    let none = Vec::new();
    let gui = vec![
        cmd("lc-edit", LaunchMode::Gui, Verb::Edit),
        cmd("lc-open", LaunchMode::Gui, Verb::Open),
    ];
    let term = vec![cmd("lc-term", LaunchMode::Terminal, Verb::Edit)];
    let mut v = Vec::new();
    for shape in [Shape::Text, Shape::Table, Shape::Document] {
        let mk = |name, verbs: Vec<VerbDecl>, toml, keys| Case {
            name,
            shape,
            verbs,
            toml,
            keys,
        };
        v.push(mk("no verb", none.clone(), "", "eo"));
        v.push(mk("gui verbs", gui.clone(), "", "eo"));
        v.push(mk("terminal verb", term.clone(), "", "e"));
        v.push(mk("external off", gui.clone(), "[external]\nopen_links = false\noffice_apps = false\n", "eo"));
        v.push(mk("editor ext explicit", none.clone(), "[editor]\ncommand = \"ed-x\"\n[editor.ext]\nlnc = \"ed-lnc\"\n", "e"));
        v.push(mk("caret moved", none.clone(), "[editor]\ncommand = \"ed-x\"\n", "jje"));
    }
    v
}

#[test]
fn e_and_o_start_the_same_things_in_the_host_and_in_the_app() {
    let dir = sandbox("launch-parity");
    std::fs::write(dir.join("a.lnc"), b"x").unwrap();
    let root = canon(&dir);
    let mut compared = 0;
    for c in cases() {
        let (h, a) = (host(&root, &c), app(&root, &c));
        let who = format!("{} / {:?}", c.name, c.shape);
        let idle = Case { keys: "", ..c.clone() };
        let (h0, a0) = (host(&root, &idle), app(&root, &idle));
        assert_eq!(h0.hints, a0.hints, "hints: {who}");
        // The host's editor is recorded as `edit:<line>`, the app's as the hand-off argv: compare
        // the number and the kind of things started, and every GUI / terminal argv exactly.
        let strip = |v: &[String]| -> Vec<String> {
            v.iter()
                .map(|s| {
                    if s.starts_with("edit:") || s.starts_with("hand:") && !s.contains("lc-") {
                        "edit".to_string()
                    } else {
                        s.replace("hand:", "term:")
                    }
                })
                .collect()
        };
        assert_eq!(strip(&h.started), strip(&a.started), "started: {who}");
        // KNOWN GAP: with `o` switched off the host flashes a notice and the app has no `o` key at
        // all (key and hint share one predicate), so no notice can be compared there.
        if let (Some(n), false) = (&h.notice, c.name == "external off") {
            let screen = a.notice.as_deref().unwrap_or("");
            assert!(
                screen.contains(n.as_str()),
                "notice {n:?} missing in the app: {who}"
            );
        }
        compared += 3;
    }
    assert!(compared >= 40, "{compared}");
}
