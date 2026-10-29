//! What a plugin declares (`Manifest::actions`, `CanvasView::text_input`) in the konoma host and
//! in the standalone app (`konoma-plugin` feature `app`). The promise: one plugin behaves the same
//! as a standalone app and inside konoma, with the same keys, footer and help.
//!
//! The plugin (`Acty`) is built here: it shows a canvas, a text or a table (the three kinds that
//! do not need files beside them), declares `bump` on `x`, `save` (a writing action) on `w` and
//! `gated` on `u` (works only while the test says so), and can ask for the typing. Every run keeps
//! a log of what the session was given, so the two sides are compared on the screen **and** on what
//! the plugin saw.
//!
//! The app is started through `prepare_bundled`, the function the real program calls, so the
//! actions come from the manifest, `[keys.<id>]` and `[plugins.<id>]` the way they do in
//! production.

use super::image_parity::{open_nested, WhenIdle};
use super::table_parity::{hints, panel_lines, rows_of, APP_H, HOST_H, W};
use super::*;
use crossterm::event::KeyCode;
use konoma_host_core::help_msg::CoreMsg;
use konoma_plugin::app::{
    key_from_crossterm, prepare_bundled, run_with_pictures, AppCx, AppEvent, Display,
};
use konoma_plugin::key::KeyCode as PK;
use konoma_plugin::prelude::*;
use konoma_plugin::view::{CanvasView, DocOptions, DocumentView, Grid, TableView};
use konoma_plugin::Origin;
use konoma_plugin::{Key, Style as PStyle, TermCaps};
use konoma_test_support::TermScreen;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Kind {
    Canvas,
    Text,
    Table,
    /// A document (the host's decorated widget); the line is its Markdown.
    Document,
}

pub(super) const KINDS: [Kind; 4] = [Kind::Canvas, Kind::Text, Kind::Table, Kind::Document];

#[derive(Clone)]
pub(super) struct Opt {
    kind: Kind,
    /// The canvas asks for the typing.
    input: bool,
    /// What `Session::enabled("gated")` answers.
    gate_on: bool,
    lang: &'static str,
    /// Config text; `{id}` is the plugin's id.
    toml: String,
    /// Declares `reload` (on `Z`): it writes `v2` into the file and answers `Outcome::Reload`;
    /// on a canvas that types, the key `!` does the same.
    reload: bool,
    /// The plugin answers its view with a `Reopen` into a second plugin (`ActyTarget`) that takes
    /// the Markdown and declares an action of its own (`tbump` on `B`).
    reopen: bool,
}

impl Opt {
    pub(super) fn new(kind: Kind) -> Opt {
        Opt {
            kind,
            input: false,
            gate_on: true,
            lang: "en",
            toml: String::new(),
            reload: false,
            reopen: false,
        }
    }
    pub(super) fn reopen(mut self) -> Opt {
        self.reopen = true;
        self
    }
    fn reload(mut self) -> Opt {
        self.reload = true;
        self
    }
    pub(super) fn lang(mut self, l: &'static str) -> Opt {
        self.lang = l;
        self
    }
    pub(super) fn gate(mut self, on: bool) -> Opt {
        self.gate_on = on;
        self
    }
    fn input(mut self) -> Opt {
        self.input = true;
        self
    }
    pub(super) fn toml(mut self, t: &str) -> Opt {
        self.toml = t.to_string();
        self
    }
}

/// What the plugin saw, in order: `act:<id>`, `key:<c>`, `key:<code>`, `paste:<bytes>`.
pub(super) type Log = Arc<Mutex<Vec<String>>>;

struct Acty {
    id: String,
    opt: Opt,
    log: Log,
    /// How many sessions were opened.
    opens: Arc<AtomicUsize>,
}

impl Plugin for Acty {
    fn manifest(&self) -> Manifest {
        let kind = match self.opt.kind {
            Kind::Canvas => ViewKind::Canvas,
            Kind::Text => ViewKind::Text,
            Kind::Table => ViewKind::Table,
            Kind::Document => ViewKind::Document,
        };
        let m = Manifest::new(self.id.as_str(), "0.1.0")
            .bind(Bind::files(["*.pnt"]))
            .shows(kind);
        let m = if self.opt.reload {
            m.action(
                Act::new("reload")
                    .key("Z")
                    .label(t!(en: "reload", ja: "再読込"))
                    .help(t!(en: "Rewrite the file and open it again", ja: "ファイルを書き換えて開き直す")),
            )
        } else {
            m
        };
        m.action(
            Act::new("bump")
                .key("x")
                .label(t!(en: "bump", ja: "加算"))
                .help(t!(en: "Add one to the counter", ja: "カウンタを 1 増やす"))
                .footer(1),
        )
        .action(
            Act::new("save")
                .key("w")
                .label(t!(en: "save", ja: "保存"))
                .help(t!(en: "Write the counter out", ja: "カウンタを書き出す"))
                .footer(2)
                .writes(),
        )
        .action(
            Act::new("gated")
                .key("u")
                .label(t!(en: "gated", ja: "条件付き"))
                .help(t!(en: "Works only sometimes", ja: "条件付きで動く"))
                .footer(3),
        )
    }

    fn open(&self, input: Input, _: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        self.opens.fetch_add(1, Ordering::SeqCst);
        let Input::File { path, .. } = input else {
            return Err(Failure::unsupported(t!(en: "a file only")));
        };
        let file = std::fs::read_to_string(&path)
            .map_err(|_| Failure::unsupported(t!(en: "the file is gone")))?
            .trim()
            .to_string();
        Ok(Box::new(ActySession {
            opt: self.opt.clone(),
            log: self.log.clone(),
            n: 0,
            typed: String::new(),
            path,
            file,
        }))
    }
}

struct ActySession {
    opt: Opt,
    log: Log,
    n: u32,
    typed: String,
    path: std::path::PathBuf,
    /// What the file held when the session was opened.
    file: String,
}

struct OneCell(String);

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
        self.0.clone()
    }
}

impl ActySession {
    /// The one line every kind shows: `n=<counter> l=<typed length> t=[<last 8 typed>]`.
    fn line(&self) -> String {
        let tail: String = {
            let c: Vec<char> = self.typed.chars().collect();
            c[c.len().saturating_sub(8)..].iter().collect()
        };
        format!(
            "n={} l={} t=[{tail}] f={}",
            self.n,
            self.typed.len(),
            self.file
        )
    }
}

impl Session for ActySession {
    fn view(&mut self, req: &ViewReq, _: &ViewCx) -> Result<View, Failure> {
        let line = self.line();
        if self.opt.reopen {
            return Ok(View::Reopen(konoma_plugin::view::Reopen {
                content_type: "markdown".into(),
                text: line,
                resources: Default::default(),
                origin: Origin::Converted {
                    from: self.path.clone(),
                },
                truncated: false,
            }));
        }
        Ok(match self.opt.kind {
            Kind::Canvas => {
                let mut c = CanvasView::new(req.cols, req.rows);
                c.set_string(0, 0, &line, PStyle::new());
                c.text_input = self.opt.input;
                View::Canvas(c)
            }
            Kind::Text => View::text([line]),
            Kind::Document => View::Document(DocumentView {
                markdown: line,
                origin: Origin::File(self.path.clone()),
                resources: Default::default(),
                options: DocOptions::default(),
                title: None,
                truncated: false,
            }),
            Kind::Table => View::Table(TableView {
                grid: Arc::new(OneCell(line)),
                title: None,
                rainbow: false,
                addressed: false,
            }),
        })
    }

    fn act(&mut self, action: &str, _: &ActCx) -> Result<Outcome, Failure> {
        self.log.lock().unwrap().push(format!("act:{action}"));
        match action {
            "bump" => self.n += 1,
            "save" => self.n += 100,
            "gated" => self.n += 1000,
            "reload" => {
                std::fs::write(&self.path, "v2").unwrap();
                return Ok(Outcome::Reload);
            }
            _ => return Ok(Outcome::Unhandled),
        }
        Ok(Outcome::Redraw)
    }

    fn key(&mut self, key: &Key, _: &ActCx) -> Result<Outcome, Failure> {
        let mut log = self.log.lock().unwrap();
        match &key.code {
            PK::Char(c) => {
                log.push(format!("key:{c}"));
                if *c == '!' && self.opt.reload {
                    std::fs::write(&self.path, "v2").unwrap();
                    return Ok(Outcome::Reload);
                }
                self.typed.push(*c);
            }
            PK::Paste(s) => {
                log.push(format!("paste:{}", s.len()));
                self.typed.push_str(s);
            }
            other => log.push(format!("key:{other:?}")),
        }
        Ok(Outcome::Redraw)
    }

    fn enabled(&self, action: &str) -> bool {
        action != "gated" || self.opt.gate_on
    }
}

/// The plugin a `Reopen` leads to: it takes Markdown, shows it as a document, and declares
/// `tbump` (on `B`). It also answers the first plugin's `bump`, to tell where an action went.
struct ActyTarget {
    id: String,
    log: Log,
}

impl Plugin for ActyTarget {
    fn manifest(&self) -> Manifest {
        Manifest::new(self.id.as_str(), "0.1.0")
            .bind(Bind::content("markdown"))
            .shows(ViewKind::Document)
            .action(
                Act::new("tbump")
                    .key("B")
                    .label(t!(en: "tbump", ja: "対象加算"))
                    .help(t!(en: "Add one in the target", ja: "対象で 1 増やす"))
                    .footer(1),
            )
            .action(
                Act::new("tsave")
                    .key("Z")
                    .label(t!(en: "tsave", ja: "対象保存"))
                    .help(t!(en: "Write in the target", ja: "対象に書き出す"))
                    .footer(2)
                    .writes(),
            )
    }
    fn open(&self, input: Input, _: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        let Input::Content { .. } = input else {
            return Err(Failure::unsupported(t!(en: "content only")));
        };
        Ok(Box::new(TargetSession {
            log: self.log.clone(),
            n: 0,
        }))
    }
}

struct TargetSession {
    log: Log,
    n: u32,
}

impl Session for TargetSession {
    fn view(&mut self, _: &ViewReq, _: &ViewCx) -> Result<View, Failure> {
        Ok(View::Document(DocumentView {
            markdown: format!("n={} l=0 t=[] f=tgt", self.n),
            origin: Origin::Converted {
                from: std::path::PathBuf::from("a.pnt"),
            },
            resources: Default::default(),
            options: DocOptions::default(),
            title: None,
            truncated: false,
        }))
    }
    fn act(&mut self, action: &str, _: &ActCx) -> Result<Outcome, Failure> {
        self.log.lock().unwrap().push(format!("tact:{action}"));
        match action {
            "tbump" => self.n += 10,
            "tsave" => self.n += 100,
            "bump" => self.n += 1,
            _ => return Ok(Outcome::Unhandled),
        }
        Ok(Outcome::Redraw)
    }
}

static SEQ: AtomicUsize = AtomicUsize::new(0);

fn new_id() -> String {
    format!("actp-{}", SEQ.fetch_add(1, Ordering::SeqCst))
}

#[derive(Clone, Debug)]
pub(super) enum Step {
    K(&'static str),
    Paste(String),
}

pub(super) use Step::{Paste, K};

/// A key press from a token (the shared tokens, and `<Sp>` for the space bar).
fn kt(t: &str) -> (KeyCode, KeyModifiers) {
    match t {
        "<Sp>" => (KeyCode::Char(' '), KeyModifiers::NONE),
        t => super::table_parity::token(t),
    }
}

pub(super) struct Run {
    pub(super) rows: Vec<String>,
    /// The program left (the host quit or went back to the tree; the app ended).
    pub(super) left: bool,
    pub(super) log: Vec<String>,
    /// How many sessions the plugin opened.
    pub(super) opens: usize,
}

fn config_text(o: &Opt, id: &str) -> String {
    format!(
        "[ui]\nlang = \"{}\"\n{}",
        o.lang,
        o.toml.replace("{id}", id)
    )
}

pub(super) fn fixture() -> (crate::test_support::TmpDir, std::path::PathBuf) {
    let dir = sandbox("plugin-act-parity");
    std::fs::write(dir.join("a.pnt"), b"x").unwrap();
    let root = canon(&dir);
    (dir, root)
}

/// The file as the tests that rewrite it start from (`v1`).
fn reset_file(root: &std::path::Path) {
    std::fs::write(root.join("a.pnt"), "v1").unwrap();
}

/// The file's content the plugin showed (`f=<content>`).
fn file_of(rows: &[String]) -> Option<String> {
    rows.iter().find_map(|r| {
        let i = r.find(" f=")?;
        Some(
            r[i + 3..]
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric())
                .collect(),
        )
    })
}

/// The host with the file open, before any step.
fn host_sim(root: &std::path::Path, o: &Opt) -> (Sim, Log, Arc<AtomicUsize>) {
    host_sim_sized(root, o, HOST_H)
}

pub(super) fn host_sim_sized(
    root: &std::path::Path,
    o: &Opt,
    h: u16,
) -> (Sim, Log, Arc<AtomicUsize>) {
    let id = new_id();
    let log = Log::default();
    let opens = Arc::new(AtomicUsize::new(0));
    let marker = crate::plugins::test_plugins::register(
        &id,
        Arc::new(Acty {
            id: id.clone(),
            opt: o.clone(),
            log: log.clone(),
            opens: opens.clone(),
        }),
    );
    let mut cfg = toml::from_str::<crate::config::Config>(&config_text(o, &id)).expect("a config");
    // The marker disables the shipped plugins the registry would otherwise offer for the file.
    let mut t = toml::Table::new();
    t.insert("enabled".into(), toml::Value::Boolean(false));
    cfg.plugins.insert(marker, t.clone());
    if o.reopen {
        let tid = format!("{id}-t");
        let m2 = crate::plugins::test_plugins::register(
            &tid,
            Arc::new(ActyTarget {
                id: tid.clone(),
                log: log.clone(),
            }),
        );
        cfg.plugins.insert(m2, t);
    }
    let mut s = Sim::with_config_sized(root, cfg, W, h);
    open_nested(&mut s, "a.pnt");
    s.draw();
    (s, log, opens)
}

pub(super) fn host(root: &std::path::Path, o: &Opt, h: u16, steps: &[Step]) -> Run {
    let (mut s, log, opens) = host_sim_sized(root, o, h);
    for st in steps {
        match st {
            K(k) => {
                let (c, m) = kt(k);
                s.press(c, m);
            }
            Paste(text) => {
                s.app.handle_paste(text.clone());
                s.draw();
            }
        }
    }
    s.term.clear().expect("clear");
    s.draw();
    let rows = rows_of(&s.term);
    let left = s.quit || counter(&rows).is_none();
    let log = log.lock().unwrap().clone();
    Run {
        rows,
        left,
        log,
        opens: opens.load(Ordering::SeqCst),
    }
}

pub(super) fn app(root: &std::path::Path, o: &Opt, h: u16, steps: &[Step], sentinel: bool) -> Run {
    let id = new_id();
    let log = Log::default();
    let opens = Arc::new(AtomicUsize::new(0));
    let plugin: Arc<dyn Plugin> = Arc::new(Acty {
        id: id.clone(),
        opt: o.clone(),
        log: log.clone(),
        opens: opens.clone(),
    });
    let (cfg, _) = (
        konoma_host_core::shared::SharedConfig::parse(&config_text(o, &id)).expect("a config"),
        (),
    );
    let bundled: Vec<Arc<dyn Plugin>> = if o.reopen {
        vec![Arc::new(ActyTarget {
            id: format!("{id}-t"),
            log: log.clone(),
        })]
    } else {
        Vec::new()
    };
    let prepared = prepare_bundled(
        vec![plugin],
        bundled,
        &root.join("a.pnt"),
        TermCaps::new(),
        (cfg, None),
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
    let mut term = Terminal::new(TermScreen::new(W, h)).unwrap();
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
    };
    let mut evs: std::collections::VecDeque<AppEvent> = steps
        .iter()
        .map(|st| match st {
            K(k) => {
                let (c, m) = kt(k);
                AppEvent::Key(key_from_crossterm(&KeyEvent::new(c, m)).expect("a key"))
            }
            Paste(t) => AppEvent::Paste(t.clone()),
        })
        .collect();
    if sentinel {
        let (c, m) = kt("z");
        evs.push_back(AppEvent::Key(
            key_from_crossterm(&KeyEvent::new(c, m)).expect("a key"),
        ));
    }
    let mut ev = WhenIdle {
        keys: evs,
        turns: 0,
    };
    run_with_pictures(&mut term, &mut *opened.session, &mut ev, &cx, &keys, None).unwrap();
    let left = sentinel && !ev.keys.is_empty();
    let buf = term.backend().1.clone();
    let mut shown = Terminal::new(ratatui::backend::TestBackend::new(
        buf.area.width,
        buf.area.height,
    ))
    .unwrap();
    shown.draw(|f| *f.buffer_mut() = buf.clone()).expect("draw");
    let log = log.lock().unwrap().clone();
    Run {
        rows: rows_of(&shown),
        left,
        log,
        opens: opens.load(Ordering::SeqCst),
    }
}

/// The plugin's line (`n=0 l=0 t=[]`) in the rows, as both sides draw it.
pub(super) fn counter(rows: &[String]) -> Option<String> {
    rows.iter().find_map(|r| {
        let b = r.as_bytes();
        (0..r.len().saturating_sub(2)).find_map(|i| {
            let ok = r.is_char_boundary(i)
                && r[i..].starts_with("n=")
                && b.get(i + 2).is_some_and(u8::is_ascii_digit)
                && (i == 0 || !b[i - 1].is_ascii_alphabetic());
            ok.then(|| {
                let rest = &r[i..];
                rest[..rest.find(']').map_or(rest.len(), |e| e + 1)].to_string()
            })
        })
    })
}

fn footer_of(rows: &[String]) -> Vec<String> {
    hints(rows.last().expect("a footer row"))
}

/// The plugin's tokens in a footer, in order, with the index of the first.
fn plugin_tokens(tokens: &[String], expect: &[String]) -> (Vec<String>, Option<usize>) {
    let got: Vec<String> = tokens
        .iter()
        .filter(|t| expect.contains(t))
        .cloned()
        .collect();
    let at = tokens.iter().position(|t| expect.contains(t));
    (got, at)
}

pub(super) fn labels(lang: &str, gate: bool) -> Vec<String> {
    let all = if lang == "ja" {
        ["x:加算", "w:保存", "u:条件付き"]
    } else {
        ["x:bump", "w:save", "u:gated"]
    };
    all.iter()
        .take(if gate { 3 } else { 2 })
        .map(|s| s.to_string())
        .collect()
}

#[test]
fn the_footer_names_the_declared_actions_in_every_view_as_the_host_does() {
    let (_dir, root) = fixture();
    let mut compared = 0;
    for kind in KINDS {
        for lang in ["en", "ja"] {
            for gate in [true, false] {
                let o = Opt::new(kind).lang(lang).gate(gate);
                let hr = host(&root, &o, HOST_H, &[]);
                let ar = app(&root, &o, APP_H, &[], true);
                assert!(!ar.left, "{kind:?}: the app ended");
                assert!(counter(&hr.rows).is_some(), "{kind:?} host: {:?}", hr.rows);
                assert!(counter(&ar.rows).is_some(), "{kind:?} app: {:?}", ar.rows);
                // All three names, in the order of the priority they declared; `gated` only
                // while the session says it works (no hint for a key that does nothing).
                let want = labels(lang, gate);
                let all = labels(lang, true);
                let (hg, hat) = plugin_tokens(&footer_of(&hr.rows), &all);
                let (ag, aat) = plugin_tokens(&footer_of(&ar.rows), &all);
                assert_eq!(hg, want, "{kind:?} {lang} gate={gate} host");
                assert_eq!(ag, want, "{kind:?} {lang} gate={gate} app");
                // Right after the first entry of the footer, on both.
                assert_eq!(hat, Some(1), "{kind:?} host footer: {:?}", hr.rows.last());
                assert_eq!(aat, Some(1), "{kind:?} app footer: {:?}", ar.rows.last());
                compared += 1;
            }
        }
    }
    assert_eq!(compared, KINDS.len() * 4);
}

/// The titles of the help sections (a title is the row after a blank one, and the first row).
fn titles(lines: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    let mut prev_blank = true;
    for l in lines {
        if l.is_empty() {
            prev_blank = true;
            continue;
        }
        if prev_blank {
            out.push(l.clone());
        }
        prev_blank = false;
    }
    out
}

#[test]
fn the_help_lists_the_declared_actions_in_every_view_as_the_host_does() {
    let (_dir, root) = fixture();
    let mut compared = 0;
    for kind in KINDS {
        for lang in ["en", "ja"] {
            for gate in [true, false] {
                let o = Opt::new(kind).lang(lang).gate(gate);
                let hr = host(&root, &o, 130, &[K("?")]);
                let ar = app(&root, &o, 129, &[K("?")], true);
                let (hl, al) = (panel_lines(&hr.rows), panel_lines(&ar.rows));
                let rows_of_acts = |lines: &[String]| -> Vec<String> {
                    let key = if lang == "ja" { "を" } else { "counter" };
                    lines.iter().filter(|l| l.contains(key)).cloned().collect()
                };
                let (hrows, arows) = (rows_of_acts(&hl), rows_of_acts(&al));
                // `bump` and `save` always; `gated` while it works.
                let n = if gate { 3 } else { 2 };
                let (hit, ait) = (
                    hl.iter().filter(|l| l.contains("x ")).count(),
                    al.iter().filter(|l| l.contains("x ")).count(),
                );
                assert!(hit >= 1 && ait >= 1, "{kind:?}: no x row");
                let sel = |lines: &[String]| -> Vec<String> {
                    lines
                        .iter()
                        .filter(|l| {
                            [
                                "Add one",
                                "Write the counter",
                                "Works only",
                                "カウンタを 1",
                                "カウンタを書",
                                "条件付きで",
                            ]
                            .iter()
                            .any(|k| l.contains(k))
                        })
                        .cloned()
                        .collect()
                };
                let (hs, asel) = (sel(&hl), sel(&al));
                assert_eq!(hs.len(), n, "{kind:?} {lang} gate={gate} host: {hl:#?}");
                assert_eq!(hs, asel, "{kind:?} {lang} gate={gate}");
                let _ = (hrows, arows);
                // The plugin's section sits right after the screen's own section, on both.
                let host_only = ["Launch", "Actions", "Tabs", "起動", "操作", "タブ"];
                let keep = |lines: &[String]| -> Vec<String> {
                    titles(lines)
                        .into_iter()
                        .filter(|t| !host_only.iter().any(|h| t.starts_with(h)))
                        .collect()
                };
                let (ht, at) = (keep(&hl), keep(&al));
                let first_row = |lines: &[String]| -> usize {
                    titles(lines)
                        .iter()
                        .position(|t| {
                            lines
                                .iter()
                                .skip_while(|l| *l != t)
                                .nth(1)
                                .is_some_and(|nxt| sel(std::slice::from_ref(nxt)).len() == 1)
                        })
                        .expect("the plugin's section")
                };
                let (hp, ap) = (first_row(&hl), first_row(&al));
                assert!(hp >= 1 && ap >= 1, "{kind:?}: the section is first");
                // The section before it is the screen's own (`Preview: ...` / a view title).
                let before = |lines: &[String], p: usize| titles(lines)[p - 1].clone();
                assert_eq!(
                    before(&hl, hp),
                    before(&al, ap),
                    "{kind:?} {lang}: the section before the plugin's differs\nhost: {ht:?}\napp: {at:?}"
                );
                compared += 1;
            }
        }
    }
    assert_eq!(compared, KINDS.len() * 4);
}

#[test]
fn a_declared_key_acts_in_every_view_and_the_plugin_sees_the_same() {
    let (_dir, root) = fixture();
    let mut compared = 0;
    for kind in KINDS {
        for (steps, want_n, want_log) in [
            (vec![K("x")], "n=1", vec!["act:bump"]),
            (vec![K("x"), K("x"), K("x")], "n=3", vec!["act:bump"; 3]),
            (vec![K("w")], "n=100", vec!["act:save"]),
            (
                vec![K("x"), K("w"), K("u")],
                "n=1101",
                vec!["act:bump", "act:save", "act:gated"],
            ),
        ] {
            let o = Opt::new(kind);
            let hr = host(&root, &o, HOST_H, &steps);
            let ar = app(&root, &o, APP_H, &steps, true);
            assert!(!hr.left && !ar.left, "{kind:?} {steps:?}: left");
            let (hc, ac) = (counter(&hr.rows).unwrap(), counter(&ar.rows).unwrap());
            assert!(hc.starts_with(want_n), "{kind:?} {steps:?} host: {hc}");
            assert_eq!(hc, ac, "{kind:?} {steps:?}");
            assert_eq!(hr.log, want_log, "{kind:?} {steps:?} host log");
            assert_eq!(ar.log, want_log, "{kind:?} {steps:?} app log");
            compared += 1;
        }
    }
    assert_eq!(compared, KINDS.len() * 4);
}

#[test]
fn an_action_the_session_does_not_enable_does_nothing_and_swallows_the_key() {
    let (_dir, root) = fixture();
    let mut compared = 0;
    for kind in KINDS {
        let o = Opt::new(kind).gate(false);
        for steps in [
            vec![K("u")],
            vec![K("u"), K("x")],
            vec![K("u"), K("u"), K("u")],
        ] {
            let hr = host(&root, &o, HOST_H, &steps);
            let ar = app(&root, &o, APP_H, &steps, true);
            assert!(!hr.left && !ar.left, "{kind:?} {steps:?}: left");
            let expect_n = if steps.len() == 2 { "n=1 " } else { "n=0 " };
            let (hc, ac) = (counter(&hr.rows).unwrap(), counter(&ar.rows).unwrap());
            assert!(hc.starts_with(expect_n), "{kind:?} {steps:?} host: {hc}");
            assert_eq!(hc, ac, "{kind:?} {steps:?}");
            // `gated` never reached the session; `bump` did.
            assert!(!hr.log.iter().any(|l| l == "act:gated"), "{:?}", hr.log);
            assert_eq!(hr.log, ar.log, "{kind:?} {steps:?} logs");
            // The key did not fall through to something else: no help, no panel.
            assert!(
                !ar.rows.iter().any(|r| r.contains("Preview:")),
                "{:?}",
                ar.rows
            );
            compared += 1;
        }
    }
    assert_eq!(compared, KINDS.len() * 3);
}

#[test]
fn a_writing_action_is_refused_for_a_read_only_plugin_as_in_the_host() {
    let (_dir, root) = fixture();
    let mut compared = 0;
    for kind in KINDS {
        for lang in ["en", "ja"] {
            let o = Opt::new(kind)
                .lang(lang)
                .toml("[plugins.\"{id}\"]\nread_only = true\n");
            let msg = if lang == "ja" {
                CoreMsg::PluginReadOnly.ja()
            } else {
                CoreMsg::PluginReadOnly.en()
            };
            let hr = host(&root, &o, HOST_H, &[K("w")]);
            let ar = app(&root, &o, APP_H, &[K("w")], false);
            for (who, r) in [("host", &hr), ("app", &ar)] {
                assert!(
                    r.rows.last().unwrap().contains(msg),
                    "{kind:?} {lang} {who}: {:?}",
                    r.rows.last()
                );
                assert!(
                    r.log.is_empty(),
                    "{who}: the writing action ran: {:?}",
                    r.log
                );
                assert!(counter(&r.rows).unwrap().starts_with("n=0 "), "{who}");
            }
            // A non-writing action still works for the same plugin.
            let hr = host(&root, &o, HOST_H, &[K("x")]);
            let ar = app(&root, &o, APP_H, &[K("x")], true);
            assert_eq!(hr.log, ["act:bump"]);
            assert_eq!(ar.log, ["act:bump"]);
            assert_eq!(counter(&hr.rows), counter(&ar.rows));
            compared += 1;
        }
    }
    assert_eq!(compared, KINDS.len() * 2);
}

#[test]
fn the_users_keys_for_a_plugin_rebind_and_free_its_actions_the_same_way() {
    let (_dir, root) = fixture();
    // `b` runs `bump`, `x` is taken from it, `i` runs `gated` (instead of `u`).
    let toml = "[keys.\"{id}\"]\nb = \"bump\"\nx = \"noop\"\ni = \"gated\"\n";
    let mut compared = 0;
    for kind in KINDS {
        for lang in ["en", "ja"] {
            let o = Opt::new(kind).lang(lang).toml(toml);
            for (steps, want) in [
                (vec![K("b")], vec!["act:bump"]),
                (vec![K("x")], vec![]),
                (vec![K("i")], vec!["act:gated"]),
                (vec![K("b"), K("i")], vec!["act:bump", "act:gated"]),
            ] {
                let hr = host(&root, &o, HOST_H, &steps);
                let ar = app(&root, &o, APP_H, &steps, true);
                assert_eq!(hr.log, want, "{kind:?} {lang} {steps:?} host");
                assert_eq!(ar.log, want, "{kind:?} {lang} {steps:?} app");
                assert_eq!(counter(&hr.rows), counter(&ar.rows), "{kind:?} {steps:?}");
                compared += 1;
            }
            // The footer names the keys the user chose.
            let (hr, ar) = (
                host(&root, &o, HOST_H, &[]),
                app(&root, &o, APP_H, &[], true),
            );
            let (bump, gated) = if lang == "ja" {
                ("b:加算", "i:条件付き")
            } else {
                ("b:bump", "i:gated")
            };
            for (who, r) in [("host", &hr), ("app", &ar)] {
                let f = footer_of(&r.rows);
                assert!(f.iter().any(|t| t == bump), "{who} {kind:?}: {f:?}");
                assert!(f.iter().any(|t| t == gated), "{who} {kind:?}: {f:?}");
                assert!(!f.iter().any(|t| t.starts_with("x:")), "{who}: {f:?}");
            }
            compared += 1;
        }
    }
    assert_eq!(compared, KINDS.len() * 10);
}

/// The typing hint in the footer while the plugin takes the keys.
#[test]
fn typing_flows_the_same_in_the_host_and_the_app() {
    let (_dir, root) = fixture();
    let mut compared = 0;
    for lang in ["en", "ja"] {
        let o = Opt::new(Kind::Canvas).lang(lang).input();
        let hint = if lang == "ja" {
            CoreMsg::StPluginInputHint.ja()
        } else {
            CoreMsg::StPluginInputHint.en()
        };
        // Typing: every key (a declared key, `q`, `?`) is the plugin's text.
        let steps = vec![K("a"), K("x"), K("q"), K("?"), K("v")];
        let hr = host(&root, &o, HOST_H, &steps);
        let ar = app(&root, &o, APP_H, &steps, false);
        assert_eq!(hr.log, ["key:a", "key:x", "key:q", "key:?", "key:v"]);
        assert_eq!(ar.log, hr.log);
        let (hc, ac) = (counter(&hr.rows).unwrap(), counter(&ar.rows).unwrap());
        assert_eq!(hc, "n=0 l=5 t=[axq?v]", "host");
        assert_eq!(hc, ac);
        for (who, r) in [("host", &hr), ("app", &ar)] {
            assert!(
                r.rows.last().unwrap().contains(hint),
                "{lang} {who} footer: {:?}",
                r.rows.last()
            );
            assert!(!r.left, "{who} left on q");
        }
        compared += 1;

        // Esc leaves the typing, not the program; the typing is not taken back by a later view
        // (the `a` after it is no text), nor by the same key.
        let steps = vec![K("a"), K("<Esc>"), K("a"), K("b")];
        let hr = host(&root, &o, HOST_H, &steps);
        let ar = app(&root, &o, APP_H, &steps, true);
        assert!(!hr.left && !ar.left, "Esc left the program");
        assert_eq!(hr.log, ["key:a", "key:Esc"], "host");
        assert_eq!(ar.log, hr.log, "app");
        assert_eq!(counter(&hr.rows), counter(&ar.rows));
        for (who, r) in [("host", &hr), ("app", &ar)] {
            assert!(
                !r.rows.last().unwrap().contains(hint),
                "{who}: the hint stays after Esc"
            );
        }
        compared += 1;

        // After Esc the declared key acts again, and the answer to it takes the typing back.
        let steps = vec![K("<Esc>"), K("x"), K("c"), K("x")];
        let hr = host(&root, &o, HOST_H, &steps);
        let ar = app(&root, &o, APP_H, &steps, false);
        assert_eq!(hr.log, ["key:Esc", "act:bump", "key:c", "key:x"], "host");
        assert_eq!(ar.log, hr.log, "app");
        assert_eq!(counter(&hr.rows), counter(&ar.rows));
        assert_eq!(counter(&hr.rows).unwrap(), "n=1 l=2 t=[cx]");
        compared += 1;

        // A second Esc (no typing now) leaves the program on both.
        let steps = vec![K("<Esc>"), K("<Esc>")];
        let hr = host(&root, &o, HOST_H, &steps);
        let ar = app(&root, &o, APP_H, &steps, true);
        assert!(hr.left && ar.left, "host {} app {}", hr.left, ar.left);
        compared += 1;
    }
    assert_eq!(compared, 8);
}

#[test]
fn a_paste_is_one_key_for_a_plugin_that_types_and_nothing_for_one_that_does_not() {
    let (_dir, root) = fixture();
    let text = "héllo\n世界 \t end".to_string();
    let n = text.len();
    let hr = host(
        &root,
        &Opt::new(Kind::Canvas).input(),
        HOST_H,
        &[Paste(text.clone())],
    );
    let ar = app(
        &root,
        &Opt::new(Kind::Canvas).input(),
        APP_H,
        &[Paste(text.clone())],
        false,
    );
    assert_eq!(hr.log, [format!("paste:{n}")]);
    assert_eq!(ar.log, hr.log);
    assert_eq!(counter(&hr.rows), counter(&ar.rows));
    assert!(counter(&hr.rows)
        .unwrap()
        .starts_with(&format!("n=0 l={n} ")));
    // Without the typing (and on a text or a table) a paste is not the plugin's.
    let mut none = 0;
    for kind in KINDS {
        let o = Opt::new(kind);
        let hr = host(&root, &o, HOST_H, &[Paste(text.clone())]);
        let ar = app(&root, &o, APP_H, &[Paste(text.clone())], true);
        assert!(hr.log.is_empty() && ar.log.is_empty(), "{kind:?}");
        assert_eq!(counter(&hr.rows), counter(&ar.rows));
        none += 1;
    }
    assert_eq!(none, KINDS.len());
}

#[test]
fn a_paste_over_the_limit_is_cut_with_a_notice_on_both() {
    let (_dir, root) = fixture();
    let mut compared = 0;
    for lang in ["en", "ja"] {
        let o = Opt::new(Kind::Canvas).lang(lang).input();
        let msg = if lang == "ja" {
            CoreMsg::PluginPasteCut.ja()
        } else {
            CoreMsg::PluginPasteCut.en()
        };
        let big = "a".repeat(1024 * 1024 + 10);
        let hr = host(&root, &o, HOST_H, &[Paste(big.clone())]);
        let ar = app(&root, &o, APP_H, &[Paste(big)], false);
        assert_eq!(hr.log, ["paste:1048576"], "host");
        assert_eq!(ar.log, hr.log, "app");
        for (who, r) in [("host", &hr), ("app", &ar)] {
            assert!(
                r.rows.last().unwrap().contains(msg),
                "{lang} {who}: {:?}",
                r.rows.last()
            );
        }
        // A paste within the limit is whole and silent.
        let hr = host(&root, &o, HOST_H, &[Paste("short".into())]);
        let ar = app(&root, &o, APP_H, &[Paste("short".into())], false);
        for (who, r) in [("host", &hr), ("app", &ar)] {
            assert_eq!(r.log, ["paste:5"], "{who}");
            assert!(!r.rows.last().unwrap().contains(msg), "{who}");
        }
        compared += 1;
    }
    assert_eq!(compared, 2);
}

/// The production start path builds the actions from the manifest, `[keys.<id>]` and
/// `[plugins.<id>]`.
#[test]
fn the_start_path_of_the_program_builds_the_actions_from_the_config() {
    let (_dir, root) = fixture();
    let prep = |toml: &str| {
        let id = new_id();
        let o = Opt::new(Kind::Canvas).toml(toml);
        let plugin: Arc<dyn Plugin> = Arc::new(Acty {
            id: id.clone(),
            opt: o.clone(),
            log: Log::default(),
            opens: Arc::default(),
        });
        let cfg = konoma_host_core::shared::SharedConfig::parse(&config_text(&o, &id)).unwrap();
        let p = prepare_bundled(
            vec![plugin],
            Vec::new(),
            &root.join("a.pnt"),
            TermCaps::new(),
            (cfg, None),
        )
        .map_err(|f| f.error)
        .expect("prepares");
        p.acts
    };
    let plain = prep("");
    let ids: Vec<&str> = plain.bindings.iter().map(|b| b.act.as_str()).collect();
    assert_eq!(ids, ["bump", "save", "gated"]);
    assert!(!plain.read_only);
    assert!(
        plain
            .bindings
            .iter()
            .find(|b| b.act == "save")
            .unwrap()
            .writes
    );
    let keyed = prep("[keys.\"{id}\"]\nb = \"bump\"\n[plugins.\"{id}\"]\nread_only = true\n");
    assert!(keyed.read_only);
    let bump = keyed.bindings.iter().find(|b| b.act == "bump").unwrap();
    assert_eq!(
        bump.key.map(|k| k.code),
        Some(KeyCode::Char('b')),
        "the user's key replaced the default"
    );
}

/// The footer text at the bottom of the screen (the flash, when there is one), with the plugin's
/// generated id (`actp-<n>`, different in each run) replaced by `ID`.
fn last_row(r: &Run) -> String {
    let mut row = r.rows.last().unwrap().trim().to_string();
    while let Some(i) = row.find("actp-") {
        let end = row[i + 5..]
            .find(|c: char| !c.is_ascii_digit())
            .map_or(row.len(), |e| i + 5 + e);
        row.replace_range(i..end, "ID");
    }
    row
}

#[test]
fn a_reload_opens_the_file_again_with_the_plugin_in_the_host_and_the_app() {
    let (_dir, root) = fixture();
    let mut compared = 0;
    for kind in KINDS {
        // (steps, the counter after, how many sessions were opened, the file shown)
        for (steps, want_n, want_opens, want_log) in [
            (vec![K("Z")], "n=0 ", 2, vec!["act:reload"]),
            (
                vec![K("x"), K("Z")],
                "n=0 ",
                2,
                vec!["act:bump", "act:reload"],
            ),
            (
                vec![K("Z"), K("x")],
                "n=1 ",
                2,
                vec!["act:reload", "act:bump"],
            ),
            (
                vec![K("x"), K("Z"), K("x"), K("x"), K("Z")],
                "n=0 ",
                3,
                vec![
                    "act:bump",
                    "act:reload",
                    "act:bump",
                    "act:bump",
                    "act:reload",
                ],
            ),
        ] {
            let o = Opt::new(kind).reload();
            reset_file(&root);
            let hr = host(&root, &o, HOST_H, &steps);
            reset_file(&root);
            let ar = app(&root, &o, APP_H, &steps, true);
            assert!(!hr.left && !ar.left, "{kind:?} {steps:?}: left");
            let (hc, ac) = (counter(&hr.rows).unwrap(), counter(&ar.rows).unwrap());
            assert!(hc.starts_with(want_n), "{kind:?} {steps:?} host: {hc}");
            assert_eq!(hc, ac, "{kind:?} {steps:?}");
            // The file was read again: the new session shows what the action wrote.
            assert_eq!(file_of(&hr.rows).as_deref(), Some("v2"), "{kind:?} host");
            assert_eq!(file_of(&ar.rows).as_deref(), Some("v2"), "{kind:?} app");
            assert_eq!(hr.opens, want_opens, "{kind:?} {steps:?} host opens");
            assert_eq!(ar.opens, want_opens, "{kind:?} {steps:?} app opens");
            assert_eq!(hr.log, want_log, "{kind:?} {steps:?} host log");
            assert_eq!(ar.log, want_log, "{kind:?} {steps:?} app log");
            compared += 1;
        }
    }
    assert_eq!(compared, KINDS.len() * 4);
}

/// A key the plugin answers with a reload (typed into a canvas): the file is opened again, the
/// typed text starts over, the keys after it reach the new session.
#[test]
fn a_reload_answered_to_a_typed_key_opens_the_file_again_on_both() {
    let (_dir, root) = fixture();
    let mut compared = 0;
    for steps in [
        vec![K("a"), K("!")],
        vec![K("a"), K("!"), K("b"), K("c")],
        vec![K("!"), K("!")],
    ] {
        let o = Opt::new(Kind::Canvas).input().reload();
        reset_file(&root);
        let hr = host(&root, &o, HOST_H, &steps);
        reset_file(&root);
        let ar = app(&root, &o, APP_H, &steps, false);
        let (hc, ac) = (counter(&hr.rows).unwrap(), counter(&ar.rows).unwrap());
        assert_eq!(hc, ac, "{steps:?}");
        assert_eq!(hr.log, ar.log, "{steps:?} logs");
        assert_eq!(hr.opens, ar.opens, "{steps:?} opens");
        assert!(ar.opens >= 2, "{steps:?}: no reload happened");
        assert_eq!(file_of(&hr.rows), file_of(&ar.rows), "{steps:?}");
        assert_eq!(file_of(&ar.rows).as_deref(), Some("v2"), "{steps:?}");
        compared += 1;
    }
    assert_eq!(compared, 3);
}

#[test]
fn the_keys_table_mistakes_are_told_in_the_footer_at_start_on_both() {
    let (_dir, root) = fixture();
    let mut compared = 0;
    let cases = [
        // (config, what the message names)
        ("[keys.\"{id}\"]\nq1 = \"nosuch\"\n", vec!["nosuch"]),
        ("[keys.\"{id}\"]\nab = \"bump\"\n", vec!["`ab`"]),
        (
            "[keys.\"{id}\"]\nq1 = \"nosuch\"\nab = \"bump\"\nzz9 = \"gated\"\n",
            vec!["nosuch", "`ab`", "`zz9`"],
        ),
    ];
    for kind in KINDS {
        for lang in ["en", "ja"] {
            for (toml, names) in &cases {
                let o = Opt::new(kind).lang(lang).toml(toml);
                let hr = host_with_notes(&root, &o);
                // No key follows (a key would take the message away).
                let ar = app(&root, &o, APP_H, &[], false);
                let (hl, al) = (last_row(&hr), last_row(&ar));
                for n in names {
                    assert!(hl.contains(n), "{kind:?} {lang} host lacks {n}: {hl}");
                    assert!(al.contains(n), "{kind:?} {lang} app lacks {n}: {al}");
                }
                let head = if lang == "ja" {
                    "プラグイン:"
                } else {
                    "plugins:"
                };
                assert!(hl.contains(head) && al.contains(head), "{hl} / {al}");
                // The wording itself is pinned (the shared text cannot drift on both sides unseen).
                let (unk, bad) = if lang == "ja" {
                    ("未知の操作", "を使えません")
                } else {
                    ("unknown action", "not used")
                };
                let want = names
                    .iter()
                    .map(|n| if n.starts_with('`') { bad } else { unk });
                for w in want {
                    assert!(hl.contains(w) && al.contains(w), "{w}: {hl} / {al}");
                }
                // The same words in the same place.
                assert_eq!(hl, al, "{kind:?} {lang}");
                compared += 1;
            }
        }
    }
    assert_eq!(compared, KINDS.len() * 6);
    // A config with no mistake says nothing.
    let o = Opt::new(Kind::Text).toml("[keys.\"{id}\"]\nb = \"bump\"\n");
    let ar = app(&root, &o, APP_H, &[], false);
    assert!(!last_row(&ar).contains("plugins:"), "{:?}", ar.rows.last());
}

/// The host as it starts: the startup message is on the footer (konoma's `main` sets it before
/// the first key; here it is put after the file is open, which the opening keys would clear).
fn host_with_notes(root: &std::path::Path, o: &Opt) -> Run {
    let id = new_id();
    let log = Log::default();
    let marker = crate::plugins::test_plugins::register(
        &id,
        Arc::new(Acty {
            id: id.clone(),
            opt: o.clone(),
            log: log.clone(),
            opens: Arc::default(),
        }),
    );
    let mut cfg = toml::from_str::<crate::config::Config>(&config_text(o, &id)).expect("a config");
    let mut t = toml::Table::new();
    t.insert("enabled".into(), toml::Value::Boolean(false));
    cfg.plugins.insert(marker, t);
    let mut s = Sim::with_config_sized(root, cfg, W, HOST_H);
    open_nested(&mut s, "a.pnt");
    s.app.flash = s.app.startup_flash(None, usize::from(W));
    s.term.clear().expect("clear");
    s.draw();
    let rows = rows_of(&s.term);
    Run {
        rows,
        left: false,
        log: Vec::new(),
        opens: 0,
    }
}

/// After a `Reopen` into another plugin the session is the destination's, so its declared
/// actions are the ones offered and run, on both sides: `B` (`tbump`) acts and reaches the
/// destination, the footer and the help name the destination's action and not the original's, and
/// the original plugin's keys (`x`, `w`, `u`) do nothing -- the destination is never sent an action
/// it did not declare.
#[test]
fn after_a_reopen_the_destinations_declared_actions_act_and_the_original_keys_do_not() {
    let (_dir, root) = fixture();
    let mut compared = 0;
    for lang in ["en", "ja"] {
        let (tag, help_text, original_help) = if lang == "ja" {
            ("B:対象加算", "対象で 1 増やす", "カウンタを 1")
        } else {
            ("B:tbump", "Add one in the target", "Add one to the counter")
        };
        for (steps, want_log, want_n) in [
            (vec![], vec![], "n=0"),
            (vec![K("B")], vec!["tact:tbump"], "n=10"),
            (vec![K("B"), K("B")], vec!["tact:tbump"; 2], "n=20"),
            (vec![K("x")], vec![], "n=0"),
            (vec![K("x"), K("B"), K("x")], vec!["tact:tbump"], "n=10"),
            (vec![K("w"), K("u")], vec![], "n=0"),
        ] {
            let o = Opt::new(Kind::Document).lang(lang).reopen();
            let hr = host(&root, &o, HOST_H, &steps);
            let ar = app(&root, &o, APP_H, &steps, true);
            assert!(!hr.left && !ar.left, "{lang} {steps:?}: left");
            for (side, r) in [("host", &hr), ("app", &ar)] {
                assert_eq!(file_of(&r.rows).as_deref(), Some("tgt"), "{side} target");
                assert_eq!(r.log, want_log, "{lang} {steps:?} {side} log");
                assert_eq!(
                    counter(&r.rows).as_deref(),
                    Some(format!("{want_n} l=0 t=[]").as_str()),
                    "{lang} {steps:?} {side}"
                );
                // The footer names the destination's action only.
                let foot = footer_of(&r.rows);
                assert!(
                    foot.iter().any(|t| t == tag),
                    "{side} {lang} footer: {foot:?}"
                );
                for gone in labels(lang, true) {
                    assert!(
                        !foot.contains(&gone),
                        "{side} {lang} footer has {gone}: {foot:?}"
                    );
                }
            }
            compared += 1;
        }
        // The help: the destination's key and text, not the original's.
        let hr = host(
            &root,
            &Opt::new(Kind::Document).lang(lang).reopen(),
            130,
            &[K("?")],
        );
        let ar = app(
            &root,
            &Opt::new(Kind::Document).lang(lang).reopen(),
            129,
            &[K("?")],
            true,
        );
        for (side, r) in [("host", &hr), ("app", &ar)] {
            let lines = panel_lines(&r.rows);
            assert!(
                lines
                    .iter()
                    .any(|l| l.contains("B ") && l.contains(help_text)),
                "{side} {lang}: {lines:#?}"
            );
            assert!(
                !lines.iter().any(|l| l.contains(original_help)),
                "{side} {lang}: {lines:#?}"
            );
        }
        compared += 1;
    }
    assert_eq!(compared, 14);
}

/// The tests below look at the host alone: what the session of a `Reopen` brings with it besides
/// the keys, the footer and the help both sides share.
fn host_after_reopen(root: &std::path::Path, toml: &str, steps: &[Step]) -> Run {
    let o = Opt::new(Kind::Document).reopen().toml(toml);
    host(root, &o, HOST_H, steps)
}

#[test]
fn after_a_reopen_the_space_menu_lists_the_destinations_actions_only() {
    let (_dir, root) = fixture();
    let mut compared = 0;
    for (lang, mine, theirs) in [
        ("en", ["tbump", "tsave"], ["gated", "save"]),
        ("ja", ["対象加算", "対象保存"], ["条件付き", "保存"]),
    ] {
        let o = Opt::new(Kind::Document).lang(lang).reopen();
        let r = host(&root, &o, 40, &[K("<Sp>")]);
        let text = r.rows.join("\n");
        for m in mine {
            assert!(text.contains(m), "{lang}: the menu has {m}\n{text}");
        }
        for t in theirs {
            // `save` is inside `tsave`: look for the menu row of the original (`w` + label).
            let row = |r: &&String| r.contains(t) && !r.contains("対象") && !r.contains("tsave");
            assert!(
                !r.rows.iter().any(|x| row(&x)),
                "{lang}: the menu has {t}\n{text}"
            );
        }
        compared += 1;
    }
    assert_eq!(compared, 2);
}

#[test]
fn after_a_reopen_the_users_keys_of_the_destination_apply_and_the_originals_do_not() {
    let (_dir, root) = fixture();
    // `[keys.<destination>]` rebinds its action; the original's table does not reach it.
    let r = host_after_reopen(
        &root,
        "[keys.\"{id}-t\"]\nu = \"tbump\"\n[keys.\"{id}\"]\nb = \"bump\"\n",
        &[K("u"), K("b"), K("B")],
    );
    assert_eq!(r.log, ["tact:tbump"], "{:?}", r.rows);
    assert_eq!(counter(&r.rows).as_deref(), Some("n=10 l=0 t=[]"));
    let foot = footer_of(&r.rows);
    assert!(
        foot.iter().any(|t| t == "u:tbump"),
        "the footer names the user's key: {foot:?}"
    );
}

#[test]
fn after_a_reopen_read_only_is_the_destinations_setting() {
    let (_dir, root) = fixture();
    // The destination is read only: its writing action is refused, the other acts.
    let r = host_after_reopen(
        &root,
        "[plugins.\"{id}-t\"]\nread_only = true\n",
        &[K("Z"), K("B")],
    );
    assert_eq!(r.log, ["tact:tbump"], "{:?}", r.rows);
    // Only the original is read only: the destination's writing action acts.
    let r = host_after_reopen(
        &root,
        "[plugins.\"{id}\"]\nread_only = true\n",
        &[K("Z"), K("B")],
    );
    assert_eq!(r.log, ["tact:tsave", "tact:tbump"], "{:?}", r.rows);
}

/// What waited for the new session of a file read again goes to it only if the plugin whose
/// session it is declared the action: an action pressed for the keys of the plugin that was on
/// screen is never sent to a session that does not know it.
#[test]
fn queued_actions_go_only_to_a_session_that_declared_them() {
    let (_dir, root) = fixture();
    let o = Opt::new(Kind::Document).reopen();
    let waiting = ["bump", "tbump", "save", "nope", "tsave"];
    let mut compared = 0;
    for (reopened, want) in [
        (true, vec!["tbump", "tsave"]),
        (false, vec!["bump", "save"]),
    ] {
        let mut s = host_sim(&root, &o).0;
        let own = {
            // The id the Sim registered is the show's own plugin.
            let (id, _, _) = s.app.hand_session_over_for_test(&[], None);
            id
        };
        let owner = reopened.then(|| format!("{own}-t"));
        let (_, kept, acting) = s.app.hand_session_over_for_test(&waiting, owner.clone());
        assert_eq!(kept, want, "reopened={reopened}");
        assert_eq!(acting, owner.unwrap_or(own), "reopened={reopened}");
        compared += 1;
    }
    assert_eq!(compared, 2);
}
