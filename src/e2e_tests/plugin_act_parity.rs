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
use konoma_plugin::view::{CanvasView, Grid, TableView};
use konoma_plugin::{Key, Style as PStyle, TermCaps};
use konoma_test_support::TermScreen;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Canvas,
    Text,
    Table,
}

const KINDS: [Kind; 3] = [Kind::Canvas, Kind::Text, Kind::Table];

#[derive(Clone)]
struct Opt {
    kind: Kind,
    /// The canvas asks for the typing.
    input: bool,
    /// What `Session::enabled("gated")` answers.
    gate_on: bool,
    lang: &'static str,
    /// Config text; `{id}` is the plugin's id.
    toml: String,
}

impl Opt {
    fn new(kind: Kind) -> Opt {
        Opt {
            kind,
            input: false,
            gate_on: true,
            lang: "en",
            toml: String::new(),
        }
    }
    fn lang(mut self, l: &'static str) -> Opt {
        self.lang = l;
        self
    }
    fn gate(mut self, on: bool) -> Opt {
        self.gate_on = on;
        self
    }
    fn input(mut self) -> Opt {
        self.input = true;
        self
    }
    fn toml(mut self, t: &str) -> Opt {
        self.toml = t.to_string();
        self
    }
}

/// What the plugin saw, in order: `act:<id>`, `key:<c>`, `key:<code>`, `paste:<bytes>`.
type Log = Arc<Mutex<Vec<String>>>;

struct Acty {
    id: String,
    opt: Opt,
    log: Log,
}

impl Plugin for Acty {
    fn manifest(&self) -> Manifest {
        let kind = match self.opt.kind {
            Kind::Canvas => ViewKind::Canvas,
            Kind::Text => ViewKind::Text,
            Kind::Table => ViewKind::Table,
        };
        Manifest::new(self.id.as_str(), "0.1.0")
            .bind(Bind::files(["*.pnt"]))
            .shows(kind)
            .action(
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

    fn open(&self, _: Input, _: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        Ok(Box::new(ActySession {
            opt: self.opt.clone(),
            log: self.log.clone(),
            n: 0,
            typed: String::new(),
        }))
    }
}

struct ActySession {
    opt: Opt,
    log: Log,
    n: u32,
    typed: String,
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
        format!("n={} l={} t=[{tail}]", self.n, self.typed.len())
    }
}

impl Session for ActySession {
    fn view(&mut self, req: &ViewReq, _: &ViewCx) -> Result<View, Failure> {
        let line = self.line();
        Ok(match self.opt.kind {
            Kind::Canvas => {
                let mut c = CanvasView::new(req.cols, req.rows);
                c.set_string(0, 0, &line, PStyle::new());
                c.text_input = self.opt.input;
                View::Canvas(c)
            }
            Kind::Text => View::text([line]),
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
            _ => return Ok(Outcome::Unhandled),
        }
        Ok(Outcome::Redraw)
    }

    fn key(&mut self, key: &Key, _: &ActCx) -> Result<Outcome, Failure> {
        let mut log = self.log.lock().unwrap();
        match &key.code {
            PK::Char(c) => {
                log.push(format!("key:{c}"));
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

static SEQ: AtomicUsize = AtomicUsize::new(0);

fn new_id() -> String {
    format!("actp-{}", SEQ.fetch_add(1, Ordering::SeqCst))
}

#[derive(Clone, Debug)]
enum Step {
    K(&'static str),
    Paste(String),
}

use Step::{Paste, K};

/// A key press from a token (the shared tokens, and `<Sp>` for the space bar).
fn kt(t: &str) -> (KeyCode, KeyModifiers) {
    match t {
        "<Sp>" => (KeyCode::Char(' '), KeyModifiers::NONE),
        t => super::table_parity::token(t),
    }
}

struct Run {
    rows: Vec<String>,
    /// The program left (the host quit or went back to the tree; the app ended).
    left: bool,
    log: Vec<String>,
}

fn config_text(o: &Opt, id: &str) -> String {
    format!(
        "[ui]\nlang = \"{}\"\n{}",
        o.lang,
        o.toml.replace("{id}", id)
    )
}

fn fixture() -> (crate::test_support::TmpDir, std::path::PathBuf) {
    let dir = sandbox("plugin-act-parity");
    std::fs::write(dir.join("a.pnt"), b"x").unwrap();
    let root = canon(&dir);
    (dir, root)
}

fn host(root: &std::path::Path, o: &Opt, h: u16, steps: &[Step]) -> Run {
    let id = new_id();
    let log = Log::default();
    let marker = crate::plugins::test_plugins::register(
        &id,
        Arc::new(Acty {
            id: id.clone(),
            opt: o.clone(),
            log: log.clone(),
        }),
    );
    let mut cfg = toml::from_str::<crate::config::Config>(&config_text(o, &id)).expect("a config");
    // The marker disables the shipped plugins the registry would otherwise offer for the file.
    let mut t = toml::Table::new();
    t.insert("enabled".into(), toml::Value::Boolean(false));
    cfg.plugins.insert(marker, t);
    let mut s = Sim::with_config_sized(root, cfg, W, h);
    open_nested(&mut s, "a.pnt");
    s.draw();
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
    Run { rows, left, log }
}

fn app(root: &std::path::Path, o: &Opt, h: u16, steps: &[Step], sentinel: bool) -> Run {
    let id = new_id();
    let log = Log::default();
    let plugin: Arc<dyn Plugin> = Arc::new(Acty {
        id: id.clone(),
        opt: o.clone(),
        log: log.clone(),
    });
    let (cfg, _) = (
        konoma_host_core::shared::SharedConfig::parse(&config_text(o, &id)).expect("a config"),
        (),
    );
    let prepared = prepare_bundled(
        vec![plugin],
        Vec::new(),
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
    }
}

/// The plugin's line (`n=0 l=0 t=[]`) in the rows, as both sides draw it.
fn counter(rows: &[String]) -> Option<String> {
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

fn labels(lang: &str, gate: bool) -> Vec<String> {
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
    assert_eq!(compared, 12);
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
    assert_eq!(compared, 12);
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
    assert_eq!(compared, 12);
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
    assert_eq!(compared, 9);
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
    assert_eq!(compared, 6);
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
    assert_eq!(compared, 30);
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
    assert_eq!(none, 3);
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
