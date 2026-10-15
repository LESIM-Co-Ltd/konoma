//! A plugin's canvas that takes the typing (`CanvasView::text_input`) and asks to be drawn again
//! (`CanvasView::redraw_after`), stage 2.5 / T2c and T2d, in the host.
//!
//! A stand-in plugin (`Typist`) records every key it is given (in order, with its modifiers) and
//! answers as the test says. The keys go through the real `handle_key`; the clock is never slept
//! on: the timer is driven by calling the "due" check with an explicit `Instant`, and the
//! asynchronous cases are ordered by a `Latch`.

use super::*;
use crate::keymap::Surface;
use crate::plugins::test_plugins::{self, Latch};
use konoma_plugin::key::{KeyCode as PK, KeyMods};
use konoma_plugin::prelude::*;
use konoma_plugin::view::CanvasView;
use konoma_plugin::{Key, Style as PStyle};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// What `Session::key` answers.
#[derive(Clone)]
enum Reply {
    Redraw,
    Unhandled,
    Flash(&'static str),
    Reload,
    /// Reload for the first key, then Redraw for the rest.
    ReloadOnce,
    /// Redraw for the first key, then the message for the rest.
    RedrawThenFlash(&'static str),
    Fail,
}

/// What the test controls and reads from outside the plugin.
struct Shared {
    /// Every key the session was given, in the order given.
    log: Mutex<Vec<Key>>,
    views: AtomicUsize,
    opens: AtomicUsize,
    /// `CanvasView::text_input` of the next canvas.
    want_input: AtomicBool,
    /// `CanvasView::redraw_after` of the next canvas.
    after: Mutex<Option<Duration>>,
    reply: Mutex<Reply>,
    /// While set, `key` waits for `latch`.
    hold_keys: AtomicBool,
    /// While set, `view` waits for `latch` (the first `n` views are let through: see `free_views`).
    hold_views: AtomicBool,
    latch: Latch,
    /// While set, `open` waits for `open_gate`.
    hold_opens: AtomicBool,
    open_gate: Latch,
    /// How many sessions were dropped (freed).
    drops: AtomicUsize,
    /// Whether the session declares two representations (`one`, `two`).
    reprs: AtomicBool,
    /// The representation each `view` was asked for, and the thread it ran on.
    asked: Mutex<Vec<(Option<String>, std::thread::ThreadId)>>,
    /// The declared actions run, in order.
    acts: Mutex<Vec<String>>,
    /// While set, `key` waits for `key_gate` (a gate of its own, apart from `latch`).
    hold_key_gate: AtomicBool,
    key_gate: Latch,
    /// Whether the session declares three pages.
    pages: AtomicBool,
    /// Whether the plugin declares actions on `j` and `Enter`.
    moves: AtomicBool,
    /// Whether the canvas answered is bigger than the area asked for.
    oversize: AtomicBool,
    /// What `key` answers to the next keys, one reply per key, before `reply` is looked at (when
    /// the line is empty `reply` answers).
    script: Mutex<std::collections::VecDeque<Reply>>,
    /// While set, `view` fails.
    fail_views: AtomicBool,
    /// The size (`cols`, `rows`) each `view` was asked for.
    sizes: Mutex<Vec<(u16, u16)>>,
}

impl Shared {
    fn new() -> Arc<Shared> {
        Arc::new(Shared {
            log: Mutex::default(),
            views: AtomicUsize::new(0),
            opens: AtomicUsize::new(0),
            want_input: AtomicBool::new(true),
            after: Mutex::default(),
            reply: Mutex::new(Reply::Redraw),
            hold_keys: AtomicBool::new(false),
            hold_views: AtomicBool::new(false),
            latch: Latch::default(),
            hold_opens: AtomicBool::new(false),
            open_gate: Latch::default(),
            drops: AtomicUsize::new(0),
            reprs: AtomicBool::new(false),
            asked: Mutex::default(),
            acts: Mutex::default(),
            hold_key_gate: AtomicBool::new(false),
            key_gate: Latch::default(),
            pages: AtomicBool::new(false),
            moves: AtomicBool::new(false),
            oversize: AtomicBool::new(false),
            script: Mutex::default(),
            fail_views: AtomicBool::new(false),
            sizes: Mutex::default(),
        })
    }

    fn log(&self) -> Vec<Key> {
        self.log.lock().unwrap().clone()
    }

    fn views(&self) -> usize {
        self.views.load(Ordering::SeqCst)
    }

    fn drops(&self) -> usize {
        self.drops.load(Ordering::SeqCst)
    }

    fn acts(&self) -> Vec<String> {
        self.acts.lock().unwrap().clone()
    }

    fn sizes(&self) -> Vec<(u16, u16)> {
        self.sizes.lock().unwrap().clone()
    }
}

struct Typist {
    id: String,
    sh: Arc<Shared>,
    /// Also declares actions on `j` and on `Enter` (keys a table binds, a canvas leaves to the
    /// plugin).
    moves: bool,
}

impl Plugin for Typist {
    fn manifest(&self) -> Manifest {
        let mut m = Manifest::new(self.id.as_str(), "0.1.0")
            .bind(Bind::files(["*.typ"]))
            .shows(ViewKind::Canvas);
        if self.moves {
            m = m
                .action(
                    Act::new("down")
                        .key("j")
                        .label(t!(en: "down", ja: "下"))
                        .help(t!(en: "Move down", ja: "下へ")),
                )
                .action(
                    Act::new("pick")
                        .key("Enter")
                        .label(t!(en: "pick", ja: "選ぶ"))
                        .help(t!(en: "Pick the item", ja: "選ぶ")),
                );
        }
        m.action(
            Act::new("refresh")
                .key("x")
                .label(t!(en: "refresh", ja: "更新"))
                .help(t!(en: "Draw again", ja: "描き直す"))
                .footer(1),
        )
        .action(
            Act::new("second")
                .key("z")
                .label(t!(en: "second", ja: "二つ目"))
                .help(t!(en: "Another action", ja: "別の操作"))
                .footer(2),
        )
    }

    fn open(&self, _: Input, _: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        let seq = self.sh.opens.fetch_add(1, Ordering::SeqCst) + 1;
        if self.sh.hold_opens.load(Ordering::SeqCst) {
            self.sh.open_gate.wait();
        }
        Ok(Box::new(TypistSession {
            sh: self.sh.clone(),
            seq,
            own: 0,
        }))
    }
}

struct TypistSession {
    sh: Arc<Shared>,
    /// Which `open` made this session (1 = the first).
    seq: usize,
    /// How many keys this very session was given.
    own: usize,
}

impl Drop for TypistSession {
    fn drop(&mut self) {
        self.sh.drops.fetch_add(1, Ordering::SeqCst);
    }
}

impl Session for TypistSession {
    fn layout(&self) -> Layout {
        let mut layout = if self.sh.reprs.load(Ordering::SeqCst) {
            Layout {
                reprs: vec![
                    ReprDecl::new("one", t!(en: "One", ja: "一")),
                    ReprDecl::new("two", t!(en: "Two", ja: "二")),
                ],
                default_repr: Some("one".into()),
                default_repr_without_pixels: Some("one".into()),
                pages: None,
            }
        } else {
            Layout::single()
        };
        if self.sh.pages.load(Ordering::SeqCst) {
            layout.pages = Some(konoma_plugin::view::PageLayout::Separate {
                count: Some(3),
                titles: Vec::new(),
            });
        }
        layout
    }

    fn view(&mut self, req: &ViewReq, _: &ViewCx) -> Result<View, Failure> {
        self.sh.views.fetch_add(1, Ordering::SeqCst);
        self.sh
            .asked
            .lock()
            .unwrap()
            .push((req.repr.clone(), std::thread::current().id()));
        self.sh.sizes.lock().unwrap().push((req.cols, req.rows));
        if self.sh.hold_views.load(Ordering::SeqCst) {
            self.sh.latch.wait();
        }
        if self.sh.fail_views.load(Ordering::SeqCst) {
            return Err(Failure::external(t!(
                en: "the view failed",
                ja: "表示に失敗しました",
            )));
        }
        let mut c = if self.sh.oversize.load(Ordering::SeqCst) {
            CanvasView::new(req.cols + 7, req.rows + 5)
        } else {
            CanvasView::new(req.cols, req.rows)
        };
        c.set_string(
            0,
            0,
            &format!(
                "keys={} sess={} own={} repr={} page={}",
                self.sh.log.lock().unwrap().len(),
                self.seq,
                self.own,
                req.repr.as_deref().unwrap_or("-"),
                req.page.map_or("-".to_string(), |p| p.to_string())
            ),
            PStyle::new(),
        );
        c.text_input = self.sh.want_input.load(Ordering::SeqCst);
        c.redraw_after = *self.sh.after.lock().unwrap();
        Ok(View::Canvas(c))
    }

    fn act(&mut self, action: &str, _: &ActCx) -> Result<Outcome, Failure> {
        self.sh.acts.lock().unwrap().push(action.to_string());
        match action {
            "refresh" | "second" | "down" | "pick" => Ok(Outcome::Redraw),
            _ => Ok(Outcome::Unhandled),
        }
    }

    fn key(&mut self, key: &Key, _: &ActCx) -> Result<Outcome, Failure> {
        if self.sh.hold_keys.load(Ordering::SeqCst) {
            self.sh.latch.wait();
        }
        if self.sh.hold_key_gate.load(Ordering::SeqCst) {
            self.sh.key_gate.wait();
        }
        self.sh.log.lock().unwrap().push(key.clone());
        self.own += 1;
        let scripted = self.sh.script.lock().unwrap().pop_front();
        let reply = if let Some(next) = scripted {
            next
        } else {
            let mut r = self.sh.reply.lock().unwrap();
            let now = r.clone();
            if matches!(now, Reply::ReloadOnce) {
                *r = Reply::Redraw;
            }
            if let Reply::RedrawThenFlash(m) = now {
                *r = Reply::Flash(m);
                drop(r);
                return Ok(Outcome::Redraw);
            }
            now
        };
        match reply {
            Reply::Redraw => Ok(Outcome::Redraw),
            Reply::Unhandled => Ok(Outcome::Unhandled),
            Reply::Flash(m) => Ok(Outcome::Flash(Tr::new(m).ja(format!("{m} (ja)")))),
            Reply::Reload | Reply::ReloadOnce => Ok(Outcome::Reload),
            Reply::RedrawThenFlash(_) => Ok(Outcome::Redraw),
            Reply::Fail => Err(Failure::external(t!(
                en: "the key failed",
                ja: "キーが失敗しました",
            ))),
        }
    }
}

// ---- helpers -----------------------------------------------------------------------------------

struct Opened {
    s: Sim,
    sh: Arc<Shared>,
    dir: crate::test_support::TmpDir,
}

fn register(name: &str, sh: &Arc<Shared>) -> String {
    test_plugins::register(
        name,
        Arc::new(Typist {
            id: name.to_string(),
            sh: sh.clone(),
            moves: sh.moves.load(Ordering::SeqCst),
        }),
    )
}

/// A session on `a.typ` (and `b.typ`), already in the preview, with the plugin set up by `setup`
/// before the file is opened.
fn open_with(name: &str, lang: &str, asynchronous: bool, setup: impl FnOnce(&Shared)) -> Opened {
    let sh = Shared::new();
    setup(&sh);
    let dir = sandbox(name);
    std::fs::write(dir.join("a.typ"), b"x").unwrap();
    std::fs::write(dir.join("b.typ"), b"y").unwrap();
    let marker = register(name, &sh);
    let mut cfg = test_plugins::marker_config(&marker);
    cfg.ui.lang = lang.into();
    let mut s = Sim::with_config_sized(&canon(&dir), cfg, 90, 26);
    if asynchronous {
        s = s.with_async_plugins();
    }
    s.select("a.typ");
    s.enter();
    if asynchronous {
        settle(&mut s);
    }
    // The first measured frame asked for the canvas again.
    s.draw();
    Opened { s, sh, dir }
}

fn open(name: &str) -> Opened {
    open_with(name, "en", false, |_| {})
}

/// Apply plugin answers until no job is out.
#[track_caller]
fn settle(s: &mut Sim) {
    for _ in 0..20 {
        if !s.app.is_media_loading() {
            return;
        }
        s.drain_plugin();
    }
    panic!("a plugin job is still out after 20 answers");
}

/// The footer line as text, taken from its spans (the drawn buffer puts a gap after each wide
/// character, so Japanese is read from the spans).
fn footer(s: &Sim) -> String {
    crate::ui::status::footer_spans(&s.app, 90)
        .iter()
        .map(|sp| sp.content.to_string())
        .collect()
}

fn chars(log: &[Key]) -> String {
    log.iter()
        .map(|k| match &k.code {
            PK::Char(c) => c.to_string(),
            other => format!("<{other:?}>"),
        })
        .collect()
}

// ---- entering and leaving the capture ------------------------------------------------------------

#[test]
fn e2e_canvas_input_is_entered_when_the_canvas_asks_for_it() {
    let o = open("ci_enter");
    assert!(o.s.app.is_plugin_input());
    assert_eq!(o.s.app.surface(), Surface::PluginInput);
    assert!(o.s.app.surface().is_text_input());
    assert!(o.s.app.key_context().is_none(), "no keymap while capturing");
}

#[test]
fn e2e_canvas_input_is_not_entered_when_the_canvas_does_not_ask() {
    let o = open_with("ci_not", "en", false, |sh| {
        sh.want_input.store(false, Ordering::SeqCst)
    });
    assert!(!o.s.app.is_plugin_input());
    assert_eq!(o.s.app.surface(), Surface::PreviewCanvas);
}

#[test]
fn e2e_canvas_input_layer_zero_keys_go_to_the_host_when_not_capturing() {
    let mut o = open_with("ci_l0_host", "en", false, |sh| {
        sh.want_input.store(false, Ordering::SeqCst)
    });
    o.s.key('?');
    assert!(o.s.app.show_help, "`?` opens the help of the host");
    assert!(o.sh.log().is_empty(), "the plugin got nothing");
}

#[test]
fn e2e_canvas_input_layer_zero_keys_are_text_while_capturing() {
    let mut o = open("ci_l0_text");
    let tabs = o.s.app.tab_count();
    for c in [
        '?', '1', '2', 't', 'q', 'Q', ' ', ',', 'x', 'j', 'y', 'e', 'o', 'p', '!', 'R', 'J',
    ] {
        o.s.key(c);
    }
    assert!(!o.s.app.show_help, "`?` did not open the help");
    assert_eq!(o.s.app.tab_count(), tabs, "`t` did not open a tab");
    assert!(o.s.app.is_canvas_preview(), "`q` did not leave the preview");
    assert!(!o.s.quit, "`Q` did not quit");
    assert!(
        o.s.app.pending_leader.is_none(),
        "Space and `,` opened no menu"
    );
    assert_eq!(chars(&o.sh.log()), "?12tqQ ,xjyeop!RJ");
    assert!(
        o.sh.log().iter().all(|k| k.mods.is_empty()),
        "plain characters carry no modifier"
    );
}

#[test]
fn e2e_canvas_input_a_declared_action_key_is_text_while_capturing() {
    let mut o = open("ci_act_text");
    let before = o.sh.views();
    o.s.key('x');
    assert_eq!(
        chars(&o.sh.log()),
        "x",
        "`x` is the plugin's text, not its action"
    );
    // The key answered Redraw: one new view, not an action.
    assert_eq!(o.sh.views(), before + 1);
}

#[test]
fn e2e_canvas_input_every_kind_of_key_is_converted() {
    use crossterm::event::{KeyCode as C, KeyModifiers as M};
    let none = KeyMods::NONE;
    let ctrl = KeyMods::CTRL;
    let alt = KeyMods::ALT;
    let shift = KeyMods::SHIFT;
    let cases: Vec<(C, M, PK, KeyMods)> = vec![
        (C::Char('a'), M::NONE, PK::Char('a'), none),
        // Shift is in the character: the bit is not reported with `Char` or `BackTab`.
        (C::Char('A'), M::SHIFT, PK::Char('A'), none),
        (C::Char('A'), M::NONE, PK::Char('A'), none),
        (C::Char('日'), M::NONE, PK::Char('日'), none),
        (C::Char('c'), M::CONTROL, PK::Char('c'), ctrl),
        (C::Char('f'), M::ALT, PK::Char('f'), alt),
        (C::Char('x'), M::CONTROL | M::ALT, PK::Char('x'), ctrl | alt),
        (C::Char('X'), M::CONTROL | M::SHIFT, PK::Char('X'), ctrl),
        (
            C::Char('z'),
            M::CONTROL | M::ALT | M::SHIFT,
            PK::Char('z'),
            ctrl | alt,
        ),
        (C::Enter, M::NONE, PK::Enter, none),
        (C::Enter, M::ALT, PK::Enter, alt),
        (C::Backspace, M::NONE, PK::Backspace, none),
        (C::Backspace, M::CONTROL, PK::Backspace, ctrl),
        (C::Delete, M::NONE, PK::Delete, none),
        (C::Tab, M::NONE, PK::Tab, none),
        (C::BackTab, M::SHIFT, PK::BackTab, none),
        (C::Left, M::NONE, PK::Left, none),
        (C::Right, M::CONTROL, PK::Right, ctrl),
        (C::Up, M::NONE, PK::Up, none),
        (C::Down, M::SHIFT, PK::Down, shift),
        (C::Home, M::NONE, PK::Home, none),
        (C::End, M::NONE, PK::End, none),
        (C::PageUp, M::NONE, PK::PageUp, none),
        (C::PageDown, M::ALT, PK::PageDown, alt),
        (C::F(1), M::NONE, PK::F(1), none),
        (C::F(5), M::SHIFT, PK::F(5), shift),
        (C::F(12), M::CONTROL, PK::F(12), ctrl),
    ];
    let mut o = open("ci_convert");
    // The converter itself, key by key (so a failure names the key)...
    for (code, mods, want, want_mods) in &cases {
        let got = crate::app::plugin_key_of(&crossterm::event::KeyEvent::new(*code, *mods))
            .unwrap_or_else(|| panic!("{code:?} has a plugin name"));
        assert_eq!(got.code, *want, "{code:?} {mods:?}");
        assert_eq!(got.mods, *want_mods, "{code:?} {mods:?}");
    }
    // ...and through the real key handler, in order.
    for (code, mods, _, _) in &cases {
        o.s.press(*code, *mods);
    }
    let log = o.sh.log();
    assert_eq!(log.len(), cases.len(), "every key reached the plugin");
    for (got, (_, _, want, want_mods)) in log.iter().zip(&cases) {
        assert_eq!((&got.code, got.mods), (want, *want_mods));
    }
}

#[test]
fn e2e_canvas_input_shift_is_in_the_character_and_never_a_separate_bit_with_it() {
    use crossterm::event::{KeyCode as C, KeyEvent, KeyModifiers as M};
    let conv = |c: C, m: M| crate::app::plugin_key_of(&KeyEvent::new(c, m)).unwrap();
    // A terminal that reports Shift (kitty protocol) and one that does not give the same key,
    // so `key == Key::char('A')` holds on both.
    assert_eq!(conv(C::Char('A'), M::SHIFT), Key::char('A'));
    assert_eq!(conv(C::Char('A'), M::NONE), Key::char('A'));
    assert_eq!(conv(C::Char('!'), M::SHIFT), Key::char('!'));
    assert_eq!(conv(C::Char('日'), M::SHIFT), Key::char('日'));
    // BackTab is Shift-Tab already.
    assert_eq!(conv(C::BackTab, M::SHIFT), Key::new(PK::BackTab));
    assert_eq!(conv(C::BackTab, M::NONE), Key::new(PK::BackTab));
    // Other modifiers stay; Shift is dropped only beside them.
    assert_eq!(conv(C::Char('X'), M::CONTROL | M::SHIFT), Key::ctrl('X'));
    assert_eq!(
        conv(C::Char('x'), M::ALT | M::SHIFT),
        Key::char('x').with_mods(KeyMods::ALT)
    );
    // Keys without a case keep Shift (Shift-Left selects, Shift-Enter is not Enter).
    assert_eq!(
        conv(C::Left, M::SHIFT),
        Key::new(PK::Left).with_mods(KeyMods::SHIFT)
    );
    assert_eq!(
        conv(C::Enter, M::SHIFT),
        Key::new(PK::Enter).with_mods(KeyMods::SHIFT)
    );
    assert_eq!(
        conv(C::F(3), M::SHIFT),
        Key::new(PK::F(3)).with_mods(KeyMods::SHIFT)
    );
}

#[test]
fn e2e_canvas_input_keys_without_a_plugin_name_are_ignored() {
    use crossterm::event::{KeyCode as C, KeyModifiers as M};
    let mut o = open("ci_unnamed");
    for code in [C::Insert, C::CapsLock, C::Null, C::ScrollLock, C::Menu] {
        o.s.press(code, M::NONE);
    }
    assert!(o.sh.log().is_empty());
    assert!(o.s.app.is_plugin_input(), "still capturing");
}

#[test]
fn e2e_canvas_input_a_paste_is_one_key_with_all_the_text() {
    let mut o = open("ci_paste");
    // What the run loop does with `Event::Paste`: accept it by the surface, then hand it over.
    assert!(crate::paste_accepted(o.s.app.surface()));
    o.s.app.handle_paste("héllo\n世界 \t end".to_string());
    assert_eq!(o.sh.log(), vec![Key::paste("héllo\n世界 \t end")]);
}

#[test]
fn e2e_canvas_a_paste_without_capture_is_not_the_plugins() {
    let mut o = open_with("ci_paste_off", "en", false, |sh| {
        sh.want_input.store(false, Ordering::SeqCst)
    });
    assert!(!crate::paste_accepted(o.s.app.surface()));
    o.s.app.handle_paste("abc".to_string());
    assert!(o.sh.log().is_empty());
}

#[test]
fn e2e_canvas_input_ctrl_c_is_a_key_for_the_plugin_like_any_other() {
    // konoma's text inputs do not treat Ctrl-C specially (the terminal is in raw mode, so it is
    // not a signal): a canvas that takes the typing gets it as `Ctrl` + `c` and nothing quits.
    let mut o = open("ci_ctrl_c");
    o.s.ctrl('c');
    assert!(!o.s.quit);
    assert_eq!(o.sh.log(), vec![Key::ctrl('c')]);
    assert!(o.s.app.is_plugin_input());
}

#[test]
fn e2e_canvas_input_esc_leaves_and_the_plugin_is_told() {
    let mut o = open("ci_esc");
    o.s.key('a');
    o.s.esc();
    assert!(!o.s.app.is_plugin_input());
    assert_eq!(o.s.app.surface(), Surface::PreviewCanvas);
    let log = o.sh.log();
    assert_eq!(
        log.last(),
        Some(&Key::new(PK::Esc)),
        "the plugin got Esc: {log:?}"
    );
    // The host's own keys are back.
    o.s.key('?');
    assert!(o.s.app.show_help);
    assert_eq!(chars(&o.sh.log()), "a<Esc>", "`?` after Esc is the host's");
}

#[test]
fn e2e_canvas_input_esc_is_not_a_request_the_plugin_can_refuse() {
    // The plugin answers Unhandled to everything, Esc included: the user is out all the same.
    let mut o = open_with("ci_esc_unh", "en", false, |sh| {
        *sh.reply.lock().unwrap() = Reply::Unhandled
    });
    o.s.esc();
    assert!(!o.s.app.is_plugin_input());
    assert_eq!(o.sh.log(), vec![Key::new(PK::Esc)]);
}

/// Make the timer fire once (a tick answered by the plugin on the spot).
fn tick(o: &mut Opened) {
    let now = Instant::now();
    o.s.app.set_canvas_redraw_at_for_test(Some(now));
    assert!(
        o.s.app.canvas_redraw_if_due(now),
        "the tick asked the plugin"
    );
    o.s.draw();
}

#[test]
fn e2e_canvas_input_only_the_user_takes_the_typing_back_after_esc() {
    let mut o = open("ci_notrap");
    o.s.esc();
    assert!(!o.s.app.is_plugin_input());
    // The plugin answered Redraw to Esc and still asks for the typing: the host does not take it.
    o.s.key('?');
    assert!(o.s.app.show_help, "the host's key, not text");
    o.s.key('q');
    assert!(!o.s.app.show_help);
    assert!(o.s.app.is_canvas_preview());
    // A plugin that flaps `text_input` from timer ticks (still asking; not asking; asking again)
    // never gets the typing back: that is not the user's doing.
    tick(&mut o);
    assert!(!o.s.app.is_plugin_input(), "a tick that still asks");
    o.sh.want_input.store(false, Ordering::SeqCst);
    tick(&mut o);
    assert!(!o.s.app.is_plugin_input());
    o.sh.want_input.store(true, Ordering::SeqCst);
    tick(&mut o);
    assert!(
        !o.s.app.is_plugin_input(),
        "false then true from a tick is not a request of the user"
    );
    // A resize (a refit) is not the user's either.
    o.s.resize(80, 20);
    o.s.draw();
    o.s.draw();
    assert!(!o.s.app.is_plugin_input(), "a refit");
    // The user presses the plugin's own action (`x`, "refresh"): the canvas it is answered with
    // asks for the typing, and that one is taken.
    o.s.key('x');
    assert!(o.s.app.is_plugin_input(), "the user's action is a request");
    let before = o.sh.log().len();
    o.s.key('?');
    assert_eq!(o.sh.log().len(), before + 1, "and `?` is text again");
}

#[test]
fn e2e_canvas_input_a_user_action_answered_without_asking_does_not_take_the_typing() {
    let mut o = open("ci_notrap_user");
    o.s.esc();
    o.sh.want_input.store(false, Ordering::SeqCst);
    o.s.key('x');
    assert!(!o.s.app.is_plugin_input());
    // The mark of that action is spent: a later tick that asks is nobody's request.
    o.sh.want_input.store(true, Ordering::SeqCst);
    tick(&mut o);
    assert!(
        !o.s.app.is_plugin_input(),
        "the earlier action is not remembered"
    );
}

#[test]
fn e2e_canvas_input_a_message_between_two_canvases_does_not_release_the_latch() {
    // The flap through a message view (which always fits) instead of a canvas that does not ask.
    let mut o = open("ci_notrap_msg");
    o.s.esc();
    // The latch survives a failure/non-canvas answer: the show is replaced by a message and back.
    o.s.app.canvas_landed_for_test(Some(None), false);
    o.s.app
        .canvas_landed_for_test(Some(Some((true, None))), false);
    assert!(!o.s.app.is_plugin_input(), "not the user's doing");
    o.s.app
        .canvas_landed_for_test(Some(Some((true, None))), true);
    assert!(o.s.app.is_plugin_input(), "the user's");
}

#[test]
fn e2e_canvas_input_a_dialog_or_help_in_front_wins_over_the_capture() {
    let mut o = open("ci_layers");
    assert_eq!(o.s.app.surface(), Surface::PluginInput);
    o.s.app.show_help = true;
    assert_eq!(o.s.app.surface(), Surface::Help, "an overlay is in front");
    o.s.app.show_help = false;
    assert_eq!(o.s.app.surface(), Surface::PluginInput);
}

#[test]
fn e2e_canvas_input_capture_belongs_to_the_preview() {
    let mut o = open("ci_tree");
    o.s.esc();
    o.s.key('q');
    assert!(matches!(o.s.app.tab.mode, Mode::Tree));
    assert_eq!(o.s.app.surface(), Surface::Tree);
    // Back in the tree the host's keys are the host's.
    o.s.key('?');
    assert!(o.s.app.show_help);
}

#[test]
fn e2e_canvas_input_reentering_the_file_takes_the_typing_again() {
    let mut o = open("ci_reenter");
    o.s.esc();
    o.s.key('q');
    o.s.select("a.typ");
    o.s.enter();
    o.s.draw();
    assert!(
        o.s.app.is_plugin_input(),
        "a new visit to the file is a new request"
    );
}

// ---- the footer ----------------------------------------------------------------------------------

#[test]
fn e2e_canvas_input_footer_says_the_plugin_takes_the_typing_and_esc_leaves() {
    let o = open("ci_footer_en");
    let f = footer(&o.s);
    assert!(f.contains("the plugin is taking your typing"), "{f}");
    assert!(f.contains("Esc:leave"), "{f}");
    assert!(o.s.screen().contains("INPUT"), "the chip names the state");
}

#[test]
fn e2e_canvas_input_footer_is_in_japanese_when_the_ui_is() {
    let o = open_with("ci_footer_ja", "ja", false, |_| {});
    let f = footer(&o.s);
    assert!(f.contains("プラグインが入力を受けています"), "{f}");
    assert!(f.contains("Esc:抜ける"), "{f}");
    let chip: String = crate::ui::status::context_spans(&o.s.app)
        .iter()
        .map(|sp| sp.content.to_string())
        .collect();
    assert!(chip.contains("入力"), "{chip}");
}

#[test]
fn e2e_canvas_input_footer_goes_back_to_the_plugins_hints_after_esc() {
    let mut o = open("ci_footer_back");
    o.s.esc();
    let f = footer(&o.s);
    assert!(!f.contains("taking your typing"), "{f}");
    assert!(
        f.contains("x:refresh"),
        "the declared action is shown again: {f}"
    );
}

#[test]
fn e2e_canvas_input_help_and_menus_are_not_reachable_while_capturing() {
    let mut o = open("ci_nohelp");
    o.s.key('?');
    o.s.key(' ');
    o.s.key(',');
    assert!(!o.s.app.show_help);
    assert!(o.s.app.pending_leader.is_none());
    assert!(
        !footer(&o.s).contains("▸"),
        "no which-key menu: {}",
        footer(&o.s)
    );
}

// ---- what the plugin answers to a key --------------------------------------------------------------

#[test]
fn e2e_canvas_input_a_redraw_answer_takes_a_new_view() {
    let mut o = open("ci_redraw");
    let before = o.sh.views();
    o.s.key('a');
    o.s.key('b');
    assert_eq!(o.sh.views(), before + 2);
    o.s.see("keys=2");
}

#[test]
fn e2e_canvas_input_unhandled_for_every_key_takes_no_view() {
    let mut o = open_with("ci_unhandled", "en", false, |sh| {
        *sh.reply.lock().unwrap() = Reply::Unhandled
    });
    let before = o.sh.views();
    o.s.keys("abc");
    assert_eq!(chars(&o.sh.log()), "abc");
    assert_eq!(o.sh.views(), before, "nothing asked for a redraw");
}

#[test]
fn e2e_canvas_input_a_flash_answer_is_shown() {
    let mut o = open_with("ci_flash", "en", false, |sh| {
        *sh.reply.lock().unwrap() = Reply::Flash("saved")
    });
    o.s.key('a');
    assert_eq!(o.s.app.flash.as_deref(), Some("saved"));
    assert!(footer(&o.s).contains("saved"));
}

#[test]
fn e2e_canvas_input_a_flash_answer_is_shown_in_japanese() {
    let mut o = open_with("ci_flash_ja", "ja", false, |sh| {
        *sh.reply.lock().unwrap() = Reply::Flash("saved")
    });
    o.s.key('a');
    assert_eq!(o.s.app.flash.as_deref(), Some("saved (ja)"));
}

#[test]
fn e2e_canvas_input_a_failed_key_is_told_like_a_failed_action() {
    let mut o = open_with("ci_fail", "en", false, |sh| {
        *sh.reply.lock().unwrap() = Reply::Fail
    });
    o.s.key('a');
    assert_eq!(o.s.app.flash.as_deref(), Some("the key failed"));
    assert!(o.s.app.is_plugin_input(), "the session is still there");
}

#[test]
fn e2e_canvas_input_a_reload_answer_reads_the_file_again() {
    let mut o = open_with("ci_reload", "en", false, |sh| {
        *sh.reply.lock().unwrap() = Reply::Reload
    });
    let opens = o.sh.opens.load(Ordering::SeqCst);
    o.s.key('a');
    assert_eq!(o.sh.opens.load(Ordering::SeqCst), opens + 1, "opened again");
}

// ---- keys are never dropped ----------------------------------------------------------------------

#[test]
fn e2e_canvas_input_keys_typed_while_a_job_holds_the_session_arrive_in_order() {
    let mut o = open_with("ci_queue", "en", true, |_| {});
    let views_before = o.sh.views();
    // The first key's job waits on the latch inside `key`.
    o.sh.hold_keys.store(true, Ordering::SeqCst);
    o.s.key('a');
    assert!(o.s.app.plugin_session_is_out_for_test());
    // While the worker holds the session, more keys come: none is dropped, none is sent yet.
    for c in ['b', 'c', 'd'] {
        o.s.key(c);
    }
    o.s.key('e');
    assert_eq!(o.s.app.plugin_key_queue_len_for_test(), 4);
    assert!(o.sh.log().is_empty(), "nothing got past the held job");
    // Release: the first job ends, its answer is applied, and the waiting keys go in the next job.
    o.sh.hold_keys.store(false, Ordering::SeqCst);
    o.sh.latch.open();
    settle(&mut o.s);
    assert_eq!(
        chars(&o.sh.log()),
        "abcde",
        "all of them, in the order typed"
    );
    assert_eq!(o.s.app.plugin_key_queue_len_for_test(), 0);
    // One view per job, not one per key: `a` and then `bcde` together.
    assert_eq!(o.sh.views() - views_before, 2, "two jobs");
    o.s.see("keys=5");
}

#[test]
fn e2e_canvas_input_a_paste_waiting_behind_a_job_keeps_its_place() {
    let mut o = open_with("ci_queue_paste", "en", true, |_| {});
    o.sh.hold_keys.store(true, Ordering::SeqCst);
    o.s.key('a');
    o.s.key('b');
    o.s.app.handle_paste("PASTE".to_string());
    o.s.key('c');
    o.sh.hold_keys.store(false, Ordering::SeqCst);
    o.sh.latch.open();
    settle(&mut o.s);
    assert_eq!(
        o.sh.log(),
        vec![
            Key::char('a'),
            Key::char('b'),
            Key::paste("PASTE"),
            Key::char('c')
        ]
    );
}

#[test]
fn e2e_canvas_input_esc_typed_behind_a_job_leaves_at_once_and_is_delivered_in_order() {
    let mut o = open_with("ci_queue_esc", "en", true, |_| {});
    o.sh.hold_keys.store(true, Ordering::SeqCst);
    o.s.key('a');
    o.s.key('b');
    o.s.esc();
    // The host is out of the capture the moment Esc is pressed, whatever the plugin is doing.
    assert!(!o.s.app.is_plugin_input());
    o.s.key('?');
    assert!(o.s.app.show_help, "the host's key");
    o.sh.hold_keys.store(false, Ordering::SeqCst);
    o.sh.latch.open();
    settle(&mut o.s);
    assert_eq!(chars(&o.sh.log()), "ab<Esc>");
}

#[test]
fn e2e_canvas_input_the_queue_goes_with_the_session_when_another_file_is_opened() {
    let mut o = open_with("ci_queue_drop", "en", true, |_| {});
    o.sh.hold_keys.store(true, Ordering::SeqCst);
    o.s.key('a');
    o.s.key('b');
    o.s.key('c');
    assert_eq!(o.s.app.plugin_key_queue_len_for_test(), 2);
    // Leave (Esc is the host's) and open the other file.
    o.s.esc();
    o.s.key('q');
    o.s.select("b.typ");
    o.s.enter();
    assert_eq!(
        o.s.app.plugin_key_queue_len_for_test(),
        0,
        "the keys of the old show are gone"
    );
    o.sh.hold_keys.store(false, Ordering::SeqCst);
    o.sh.latch.open();
    // The old job answers (stale: the preview has moved on); wait for exactly that answer.
    o.s.drain_plugin();
    settle(&mut o.s);
    assert_eq!(
        chars(&o.sh.log()),
        "a",
        "only the key that was already out; the queued ones did not reach another file's session"
    );
}

#[test]
fn e2e_canvas_input_without_a_worker_every_key_is_delivered_at_once() {
    let mut o = open("ci_sync");
    o.s.keys("hello");
    assert_eq!(chars(&o.sh.log()), "hello");
    assert_eq!(o.s.app.plugin_key_queue_len_for_test(), 0);
}

// ---- the redraw timer ----------------------------------------------------------------------------

const AFTER: Duration = Duration::from_millis(250);

fn ticking(name: &str) -> Opened {
    // The canvas on screen asks to be drawn again; it does not take the typing (the two are
    // independent), so the keys stay the host's.
    let mut o = open_with(name, "en", false, |sh| {
        sh.want_input.store(false, Ordering::SeqCst);
        *sh.after.lock().unwrap() = Some(AFTER);
    });
    // One more answer so the deadline is armed from a canvas that has `redraw_after`.
    o.s.key('x');
    o
}

#[test]
fn e2e_canvas_timer_arms_a_deadline_from_the_answer() {
    let before = Instant::now();
    let o = ticking("ct_arm");
    let at = o.s.app.canvas_redraw_deadline().expect("armed");
    assert!(at >= before + AFTER, "not earlier than the plugin asked");
    assert!(at <= Instant::now() + AFTER, "not later either");
}

#[test]
fn e2e_canvas_timer_not_due_makes_no_request_and_due_makes_exactly_one() {
    let mut o = ticking("ct_due");
    let at = o.s.app.canvas_redraw_deadline().unwrap();
    let views = o.sh.views();
    assert!(!o.s.app.canvas_redraw_if_due(at - Duration::from_millis(1)));
    assert_eq!(o.sh.views(), views, "not due: nothing asked");
    assert_eq!(
        o.s.app.canvas_redraw_deadline(),
        Some(at),
        "and still armed"
    );
    assert!(o.s.app.canvas_redraw_if_due(at), "due: asked");
    assert_eq!(o.sh.views(), views + 1, "exactly one request");
    // The answer armed the next deadline; asking again at the same instant does not ask twice.
    let next =
        o.s.app
            .canvas_redraw_deadline()
            .expect("armed again by the answer");
    assert!(next > at);
    assert!(!o.s.app.canvas_redraw_if_due(at));
    assert_eq!(o.sh.views(), views + 1);
}

#[test]
fn e2e_canvas_timer_a_late_tick_is_still_one_request() {
    let mut o = ticking("ct_late");
    let at = o.s.app.canvas_redraw_deadline().unwrap();
    let views = o.sh.views();
    // The loop was busy for ten intervals: one request, not ten.
    assert!(o.s.app.canvas_redraw_if_due(at + AFTER * 10));
    assert_eq!(o.sh.views(), views + 1);
}

#[test]
fn e2e_canvas_timer_never_asks_for_less_than_33_milliseconds() {
    for (i, ask) in [
        Duration::ZERO,
        Duration::from_millis(1),
        Duration::from_millis(32),
    ]
    .into_iter()
    .enumerate()
    {
        let mut o = open_with(&format!("ct_min_{i}"), "en", false, |sh| {
            sh.want_input.store(false, Ordering::SeqCst);
            *sh.after.lock().unwrap() = Some(ask);
        });
        let before = Instant::now();
        o.s.key('x');
        let at = o.s.app.canvas_redraw_deadline().expect("armed");
        assert!(
            at >= before + Duration::from_millis(33),
            "asked {ask:?}: the deadline is {:?} after the answer",
            at.saturating_duration_since(before)
        );
    }
    // A longer ask is kept as it is.
    let o = ticking("ct_min_long");
    let at = o.s.app.canvas_redraw_deadline().unwrap();
    assert!(at >= Instant::now() - Duration::from_secs(1) + AFTER);
}

#[test]
fn e2e_canvas_timer_a_canvas_without_redraw_after_has_no_deadline_and_cancels_one() {
    let mut o = ticking("ct_cancel");
    assert!(o.s.app.canvas_redraw_deadline().is_some());
    *o.sh.after.lock().unwrap() = None;
    o.s.key('x');
    assert_eq!(
        o.s.app.canvas_redraw_deadline(),
        None,
        "the new canvas cancels it"
    );
    let views = o.sh.views();
    assert!(!o
        .s
        .app
        .canvas_redraw_if_due(Instant::now() + Duration::from_secs(60)));
    assert_eq!(o.sh.views(), views);
    assert_eq!(o.s.app.canvas_poll_timeout(Instant::now()), None);
}

#[test]
fn e2e_canvas_timer_no_redraw_after_from_the_start_means_no_deadline() {
    let o = open_with("ct_none", "en", false, |sh| {
        sh.want_input.store(false, Ordering::SeqCst)
    });
    assert_eq!(o.s.app.canvas_redraw_deadline(), None);
    assert_eq!(o.s.app.canvas_poll_timeout(Instant::now()), None);
}

#[test]
fn e2e_canvas_timer_is_not_run_in_the_tree() {
    let mut o = ticking("ct_tree");
    let at = o.s.app.canvas_redraw_deadline().unwrap();
    o.s.key('q');
    assert!(matches!(o.s.app.tab.mode, Mode::Tree));
    // (leaving drops the show and its deadline; put one back, as a stale one would be, to see that
    // the visibility check alone refuses it)
    assert_eq!(o.s.app.canvas_redraw_deadline(), None);
    o.s.app.set_canvas_redraw_at_for_test(Some(at));
    let views = o.sh.views();
    assert_eq!(
        o.s.app.canvas_poll_timeout(at),
        None,
        "the loop is not woken for a canvas nobody sees"
    );
    assert!(!o.s.app.canvas_redraw_if_due(at + AFTER));
    assert_eq!(o.sh.views(), views, "no request");
    assert_eq!(
        o.s.app.canvas_redraw_deadline(),
        None,
        "and the deadline is forgotten"
    );
}

#[test]
fn e2e_canvas_timer_returning_to_the_preview_draws_it_again_and_ticks_again() {
    let mut o = ticking("ct_return");
    o.s.key('q');
    let _ = o.s.app.canvas_redraw_if_due(Instant::now() + AFTER);
    assert_eq!(o.s.app.canvas_redraw_deadline(), None);
    o.s.select("a.typ");
    o.s.enter();
    o.s.draw();
    // Returning asks for the view as it normally does; its answer has `redraw_after`.
    assert!(
        o.s.app.canvas_redraw_deadline().is_some(),
        "the canvas that came back is on the timer again"
    );
}

#[test]
fn e2e_canvas_timer_another_tab_stops_it() {
    let mut o = ticking("ct_tab");
    let at = o.s.app.canvas_redraw_deadline().unwrap();
    // A second tab (in the tree) is now the active one.
    o.s.key('t');
    assert!(matches!(o.s.app.tab.mode, Mode::Tree));
    o.s.app.set_canvas_redraw_at_for_test(Some(at));
    let views = o.sh.views();
    assert_eq!(o.s.app.canvas_poll_timeout(at), None);
    assert!(!o.s.app.canvas_redraw_if_due(at + AFTER));
    assert_eq!(o.sh.views(), views);
}

#[test]
fn e2e_canvas_timer_opening_another_file_drops_the_old_deadline() {
    let mut o = ticking("ct_other");
    o.s.key('q');
    o.s.select("b.typ");
    // Opening the other file starts a new show; the old deadline is not carried over. The new
    // canvas arms its own, from its own answer.
    o.sh.want_input.store(false, Ordering::SeqCst);
    *o.sh.after.lock().unwrap() = None;
    o.s.enter();
    o.s.draw();
    assert_eq!(o.s.app.canvas_redraw_deadline(), None);
}

#[test]
fn e2e_canvas_timer_another_kind_of_preview_stops_it() {
    let mut o = ticking("ct_text");
    let at = o.s.app.canvas_redraw_deadline().unwrap();
    o.s.key('q');
    // A plain text file: nothing of the canvas is on screen.
    std::fs::write(o.dir.join("c.txt"), b"hello").unwrap();
    o.s.app.refresh().unwrap();
    o.s.select("c.txt");
    o.s.enter();
    o.s.app.set_canvas_redraw_at_for_test(Some(at));
    assert_eq!(
        o.s.app.canvas_poll_timeout(at),
        None,
        "no wake-up for another kind of preview"
    );
    let views = o.sh.views();
    assert!(!o.s.app.canvas_redraw_if_due(at + AFTER));
    assert_eq!(o.sh.views(), views);
    assert_eq!(o.s.app.canvas_redraw_deadline(), None);
}

#[test]
fn e2e_canvas_timer_waits_under_an_overlay_and_asks_once_when_it_closes() {
    let mut o = ticking("ct_help");
    o.s.key('?');
    assert!(o.s.app.show_help);
    let at = o.s.app.canvas_redraw_deadline().unwrap();
    let views = o.sh.views();
    // The canvas is hidden: no wake-up for it and no call of the plugin, however late.
    assert_eq!(o.s.app.canvas_poll_timeout(at), None);
    assert!(!o.s.app.canvas_redraw_if_due(at + AFTER * 10));
    assert!(!o.s.app.canvas_redraw_if_due(at + AFTER * 20));
    assert_eq!(o.sh.views(), views, "the plugin was not called under help");
    assert_eq!(
        o.s.app.canvas_redraw_deadline(),
        Some(at),
        "the deadline waits, it is not dropped"
    );
    // The overlay closes: the long-past deadline fires, once.
    o.s.key('?');
    assert!(!o.s.app.show_help);
    assert!(o.s.app.canvas_poll_timeout(at).is_some());
    assert!(o.s.app.canvas_redraw_if_due(at + AFTER * 20));
    assert_eq!(o.sh.views(), views + 1, "asked once");
    // The answer armed the next deadline from now: nothing is due at the old time.
    assert!(!o.s.app.canvas_redraw_if_due(at), "only once");
    assert_eq!(o.sh.views(), views + 1);
}

#[test]
fn e2e_canvas_timer_skips_the_tick_while_a_job_is_out_and_the_next_answer_arms_again() {
    let mut o = open_with("ct_skip", "en", true, |sh| {
        sh.want_input.store(false, Ordering::SeqCst);
        *sh.after.lock().unwrap() = Some(AFTER);
    });
    // An action whose view request is held on the worker.
    o.sh.hold_views.store(true, Ordering::SeqCst);
    o.s.key('x');
    // The action's own job ends first (its answer is applied here) and asks for the view, which
    // is the job that is held.
    o.s.drain_plugin();
    assert!(
        o.s.app.plugin_session_is_out_for_test(),
        "a job holds the session"
    );
    o.s.app.set_canvas_redraw_at_for_test(Some(Instant::now()));
    let views = o.sh.views();
    assert!(
        !o.s.app
            .canvas_redraw_if_due(Instant::now() + Duration::from_secs(1)),
        "skipped: a job is out"
    );
    assert_eq!(o.sh.views(), views, "nothing piled up");
    assert_eq!(
        o.s.app.canvas_redraw_deadline(),
        None,
        "and nothing is armed meanwhile"
    );
    // The held job answers: that answer arms the deadline again.
    o.sh.hold_views.store(false, Ordering::SeqCst);
    o.sh.latch.open();
    settle(&mut o.s);
    assert!(
        o.s.app.canvas_redraw_deadline().is_some(),
        "re-armed by the answer"
    );
}

#[test]
fn e2e_canvas_timer_a_result_without_a_view_arms_a_deadline_that_was_skipped() {
    // The tick passed while a job was out, so the deadline was taken; the job (here a key that
    // changed nothing) answers without a view: the canvas on screen is still one that wants the
    // timer, so it is armed again from it.
    let mut o = open_with("ct_rearm", "en", false, |sh| {
        sh.want_input.store(true, Ordering::SeqCst);
        *sh.after.lock().unwrap() = Some(AFTER);
        *sh.reply.lock().unwrap() = Reply::Unhandled;
    });
    o.s.key('a');
    o.s.app.set_canvas_redraw_at_for_test(None);
    o.s.key('b');
    assert!(o.s.app.canvas_redraw_deadline().is_some());
}

#[test]
fn e2e_canvas_timer_a_pending_deadline_is_not_pushed_back_by_a_result_without_a_view() {
    let mut o = open_with("ct_keep", "en", false, |sh| {
        *sh.after.lock().unwrap() = Some(AFTER);
        *sh.reply.lock().unwrap() = Reply::Unhandled;
    });
    o.s.key('a');
    let at = o.s.app.canvas_redraw_deadline().expect("armed by the view");
    o.s.key('b');
    assert_eq!(o.s.app.canvas_redraw_deadline(), Some(at));
}

#[test]
fn e2e_canvas_timer_poll_timeout_is_until_the_deadline_and_never_more_than_the_idle_wait() {
    let o = ticking("ct_poll");
    let at = o.s.app.canvas_redraw_deadline().unwrap();
    assert_eq!(
        o.s.app.canvas_poll_timeout(at - Duration::from_millis(40)),
        Some(Duration::from_millis(40))
    );
    assert_eq!(
        o.s.app.canvas_poll_timeout(at - Duration::from_secs(60)),
        Some(Duration::from_millis(100)),
        "the loop still wakes for everything else"
    );
    assert_eq!(
        o.s.app.canvas_poll_timeout(at + Duration::from_secs(1)),
        Some(Duration::ZERO),
        "overdue: wake at once"
    );
}

#[test]
fn e2e_canvas_timer_the_run_loops_wait_includes_the_deadline() {
    let mut o = ticking("ct_loop_wait");
    // Idle: the plain wait of the loop.
    o.s.app.set_canvas_redraw_at_for_test(None);
    assert_eq!(crate::poll_timeout(&o.s.app), Duration::from_millis(100));
    // A deadline that has passed: the loop wakes at once, to draw on time.
    o.s.app
        .set_canvas_redraw_at_for_test(Some(Instant::now() - Duration::from_secs(1)));
    assert_eq!(crate::poll_timeout(&o.s.app), Duration::ZERO);
    // One that is soon: at most that long.
    o.s.app
        .set_canvas_redraw_at_for_test(Some(Instant::now() + Duration::from_millis(30)));
    assert!(crate::poll_timeout(&o.s.app) <= Duration::from_millis(30));
    // The canvas not on screen: the loop idles as it does today.
    o.s.key('q');
    o.s.app
        .set_canvas_redraw_at_for_test(Some(Instant::now() - Duration::from_secs(1)));
    assert_eq!(crate::poll_timeout(&o.s.app), Duration::from_millis(100));
}

#[test]
fn e2e_canvas_timer_and_text_input_work_together() {
    let mut o = open_with("ct_both", "en", false, |sh| {
        *sh.after.lock().unwrap() = Some(AFTER);
    });
    o.s.key('a');
    assert!(o.s.app.is_plugin_input());
    let at = o.s.app.canvas_redraw_deadline().expect("armed");
    let views = o.sh.views();
    assert!(o.s.app.canvas_redraw_if_due(at));
    assert_eq!(o.sh.views(), views + 1);
    assert!(
        o.s.app.is_plugin_input(),
        "still capturing after a timer redraw"
    );
}

// ---- the job road: a reload, a wish and a press never lose the session ---------------------------

/// What a test file looks like after the file on disk changed (the size is what the host sees).
fn touch_file(o: &Opened) {
    std::fs::write(o.dir.join("a.typ"), b"changed, and longer").unwrap();
}

#[test]
fn e2e_canvas_reload_the_old_session_is_out_of_reach_while_the_file_is_read_again() {
    // The previous view stays on screen while the file is read again, but the session that made it
    // must not be taken by the timer, the keys or a refit meanwhile: whatever it answered would
    // make the new `Open` stale, and the new session (the new content) would be lost.
    let mut o = open_with("cj_reopen", "en", true, |sh| {
        *sh.after.lock().unwrap() = Some(AFTER);
    });
    o.s.see("sess=1");
    // The reading again is held in `open`.
    o.sh.hold_opens.store(true, Ordering::SeqCst);
    touch_file(&o);
    o.s.app.reload_preview();
    assert!(
        o.s.app.plugin_session_is_out_for_test(),
        "the old session is out of reach"
    );
    assert_eq!(o.sh.drops(), 1, "and it is freed (off the UI thread)");
    o.s.see("sess=1"); // the old view is still what is drawn
                       // The timer is due: nobody to ask.
    let views = o.sh.views();
    assert!(!o.s.app.canvas_redraw_if_due(Instant::now() + AFTER * 4));
    // Typed keys wait, in order.
    o.s.key('a');
    o.s.key('b');
    assert_eq!(o.s.app.plugin_key_queue_len_for_test(), 2);
    assert!(o.sh.log().is_empty(), "no key went to the old session");
    assert_eq!(o.sh.views(), views, "nothing asked the old session");
    // The new session lands: the new content is shown, and the keys go to it, in order.
    o.sh.hold_opens.store(false, Ordering::SeqCst);
    o.sh.open_gate.open();
    settle(&mut o.s);
    assert_eq!(o.sh.opens.load(Ordering::SeqCst), 2);
    o.s.see("sess=2");
    o.s.dont_see("sess=1");
    assert_eq!(chars(&o.sh.log()), "ab");
    o.s.see("own=2");
    assert_eq!(o.s.app.plugin_key_queue_len_for_test(), 0);
    // The timer re-arms from the new answer.
    assert!(o.s.app.canvas_redraw_deadline().is_some());
}

#[test]
fn e2e_canvas_reload_a_late_answer_of_the_old_session_is_not_put_back() {
    // A tick job holds the old session when the file changes. Its answer is older than the `Open`
    // and arrives first: it must not become the show's session (it would take the queued keys, and
    // the `Open` would be stale).
    let mut o = open_with("cj_late", "en", true, |sh| {
        *sh.after.lock().unwrap() = Some(AFTER);
    });
    o.sh.hold_views.store(true, Ordering::SeqCst);
    let now = Instant::now() + AFTER * 4;
    o.s.app.set_canvas_redraw_at_for_test(Some(Instant::now()));
    assert!(
        o.s.app.canvas_redraw_if_due(now),
        "the tick is out with the old session"
    );
    o.sh.hold_views.store(false, Ordering::SeqCst);
    // The file changes: read again; the `Open` waits in `open`.
    o.sh.hold_opens.store(true, Ordering::SeqCst);
    touch_file(&o);
    o.s.app.reload_preview();
    o.s.key('a');
    assert_eq!(o.s.app.plugin_key_queue_len_for_test(), 1);
    // The tick finishes first: its answer is stale, and its session is not given back.
    o.sh.latch.open();
    assert!(!o.s.drain_plugin(), "the stale answer is not applied");
    assert!(o.s.app.plugin_session_is_out_for_test(), "not put back");
    assert_eq!(
        o.s.app.plugin_key_queue_len_for_test(),
        1,
        "the key still waits"
    );
    assert!(o.sh.log().is_empty(), "the old session was given no key");
    // Now the new session.
    o.sh.hold_opens.store(false, Ordering::SeqCst);
    o.sh.open_gate.open();
    settle(&mut o.s);
    o.s.see("sess=2");
    o.s.see("own=1");
    assert_eq!(chars(&o.sh.log()), "a");
}

#[test]
fn e2e_canvas_reload_answered_to_a_key_with_more_keys_waiting_goes_to_the_new_session() {
    // The deterministic case: the key answers `Reload` while other keys are queued. The reading
    // again and the sending of the queue happen in the same step; the queue must wait for the new
    // session, not take the old one.
    let mut o = open_with("cj_reload_keys", "en", true, |sh| {
        *sh.reply.lock().unwrap() = Reply::ReloadOnce;
    });
    o.sh.hold_keys.store(true, Ordering::SeqCst);
    o.s.key('a');
    o.s.key('b');
    o.s.key('c');
    assert_eq!(o.s.app.plugin_key_queue_len_for_test(), 2);
    o.sh.hold_keys.store(false, Ordering::SeqCst);
    // The reading again is held, so the key answer is applied alone.
    o.sh.hold_opens.store(true, Ordering::SeqCst);
    o.sh.latch.open();
    assert!(o.s.drain_plugin(), "the key answered Reload");
    assert!(
        o.s.app.plugin_session_is_out_for_test(),
        "the old session is not used again"
    );
    assert_eq!(chars(&o.sh.log()), "a", "`b` and `c` wait for the new one");
    assert_eq!(o.s.app.plugin_key_queue_len_for_test(), 2);
    o.sh.hold_opens.store(false, Ordering::SeqCst);
    o.sh.open_gate.open();
    settle(&mut o.s);
    assert_eq!(
        o.sh.opens.load(Ordering::SeqCst),
        2,
        "the file was read again"
    );
    assert_eq!(o.sh.drops(), 1, "the old session is gone");
    // The new session is what is shown, and it got `b` and `c`.
    o.s.see("sess=2");
    o.s.see("own=2");
    assert_eq!(chars(&o.sh.log()), "abc");
    assert_eq!(o.s.app.plugin_key_queue_len_for_test(), 0);
}

#[test]
fn e2e_canvas_r_is_asked_on_a_worker_never_on_the_ui_thread() {
    let mut o = open_with("cj_r", "en", true, |sh| {
        sh.reprs.store(true, Ordering::SeqCst);
        sh.want_input.store(false, Ordering::SeqCst);
    });
    assert!(footer(&o.s).contains("R:Two"), "footer: {}", footer(&o.s));
    o.s.key('R');
    settle(&mut o.s);
    o.s.see("repr=two");
    let (repr, thread) = o.sh.asked.lock().unwrap().last().cloned().unwrap();
    assert_eq!(repr.as_deref(), Some("two"));
    assert_ne!(thread, std::thread::current().id(), "not the UI thread");
}

#[test]
fn e2e_canvas_r_pressed_while_a_job_holds_the_session_is_asked_when_it_returns() {
    let mut o = open_with("cj_r_wait", "en", true, |sh| {
        sh.reprs.store(true, Ordering::SeqCst);
        sh.want_input.store(false, Ordering::SeqCst);
        *sh.after.lock().unwrap() = Some(AFTER);
    });
    o.sh.hold_views.store(true, Ordering::SeqCst);
    o.s.app.set_canvas_redraw_at_for_test(Some(Instant::now()));
    assert!(o.s.app.canvas_redraw_if_due(Instant::now() + AFTER * 4));
    o.sh.hold_views.store(false, Ordering::SeqCst);
    assert!(o.s.app.plugin_session_is_out_for_test());
    o.s.key('R');
    let two = |o: &Opened| {
        o.sh.asked
            .lock()
            .unwrap()
            .iter()
            .filter(|(r, _)| r.as_deref() == Some("two"))
            .count()
    };
    assert_eq!(two(&o), 0, "nothing asked for `two` yet");
    o.sh.latch.open();
    settle(&mut o.s);
    o.s.see("repr=two");
    // It was asked once, with the choice the tab then had.
    assert_eq!(
        o.sh.asked.lock().unwrap().last().unwrap().0.as_deref(),
        Some("two")
    );
}

#[test]
fn e2e_canvas_a_page_turn_while_a_job_holds_the_session_does_not_open_the_file_again() {
    let mut o = open_with("cj_page", "en", true, |sh| {
        sh.pages.store(true, Ordering::SeqCst);
        sh.want_input.store(false, Ordering::SeqCst);
        *sh.after.lock().unwrap() = Some(AFTER);
    });
    o.s.see("page=0");
    o.sh.hold_views.store(true, Ordering::SeqCst);
    o.s.app.set_canvas_redraw_at_for_test(Some(Instant::now()));
    assert!(o.s.app.canvas_redraw_if_due(Instant::now() + AFTER * 4));
    o.sh.hold_views.store(false, Ordering::SeqCst);
    let opens = o.sh.opens.load(Ordering::SeqCst);
    o.s.key('J');
    assert_eq!(o.sh.opens.load(Ordering::SeqCst), opens, "not opened again");
    o.s.see("page=0"); // the view stays on screen: no "Loading..."
    o.sh.latch.open();
    settle(&mut o.s);
    assert_eq!(
        o.sh.opens.load(Ordering::SeqCst),
        opens,
        "still the same session"
    );
    o.s.see("page=1");
    o.s.see("sess=1");
}

#[test]
fn e2e_canvas_actions_pressed_while_a_job_holds_the_session_wait_in_order() {
    let mut o = open_with("cj_acts", "en", true, |sh| {
        sh.want_input.store(false, Ordering::SeqCst);
        *sh.after.lock().unwrap() = Some(AFTER);
    });
    o.sh.hold_views.store(true, Ordering::SeqCst);
    o.s.app.set_canvas_redraw_at_for_test(Some(Instant::now()));
    assert!(o.s.app.canvas_redraw_if_due(Instant::now() + AFTER * 4));
    o.sh.hold_views.store(false, Ordering::SeqCst);
    assert!(o.s.app.plugin_session_is_out_for_test());
    // The hint does not flicker with the tick: the last known answer stands.
    o.s.draw();
    assert!(
        footer(&o.s).contains("x:refresh"),
        "footer: {}",
        footer(&o.s)
    );
    o.s.key('z');
    o.s.key('x');
    o.s.key('z');
    assert!(o.sh.acts().is_empty(), "nothing ran yet");
    o.sh.latch.open();
    settle(&mut o.s);
    assert_eq!(
        o.sh.acts(),
        ["second", "refresh", "second"],
        "in the order pressed"
    );
}

#[test]
fn e2e_canvas_actions_waiting_are_bounded() {
    let mut o = open_with("cj_acts_cap", "en", true, |sh| {
        sh.want_input.store(false, Ordering::SeqCst);
        *sh.after.lock().unwrap() = Some(AFTER);
    });
    o.sh.hold_views.store(true, Ordering::SeqCst);
    o.s.app.set_canvas_redraw_at_for_test(Some(Instant::now()));
    assert!(o.s.app.canvas_redraw_if_due(Instant::now() + AFTER * 4));
    o.sh.hold_views.store(false, Ordering::SeqCst);
    let cap = crate::app::MAX_QUEUED_ACTS;
    for _ in 0..cap + 10 {
        o.s.key('x');
    }
    assert!(o.s.app.flash.is_some(), "the refused press is told");
    o.sh.latch.open();
    // One action per job, each answered with a redraw: settle until the line is empty.
    for _ in 0..(cap * 3) {
        if !o.s.app.is_media_loading() {
            break;
        }
        o.s.drain_plugin();
    }
    assert_eq!(
        o.sh.acts().len(),
        cap,
        "the first {cap} ran, the rest were refused"
    );
}

#[test]
fn e2e_canvas_a_stale_keys_answer_with_a_redraw_and_a_message_keeps_both() {
    let mut o = open_with("cj_stale_keys", "en", true, |sh| {
        *sh.after.lock().unwrap() = Some(AFTER);
        *sh.reply.lock().unwrap() = Reply::RedrawThenFlash("the message");
    });
    // Two keys wait behind a tick, so they go in one job.
    o.sh.hold_views.store(true, Ordering::SeqCst);
    o.s.app.set_canvas_redraw_at_for_test(Some(Instant::now()));
    assert!(o.s.app.canvas_redraw_if_due(Instant::now() + AFTER * 4));
    o.sh.hold_views.store(false, Ordering::SeqCst);
    o.s.key('a');
    o.s.key('b');
    o.sh.hold_key_gate.store(true, Ordering::SeqCst);
    o.sh.latch.open();
    assert!(
        o.s.drain_plugin(),
        "the tick lands and the keys go in one job"
    );
    // Another media job moves the generation on while the keys are out: their answer is stale.
    o.s.app.another_media_job_starts_for_test();
    let views = o.sh.views();
    o.sh.hold_key_gate.store(false, Ordering::SeqCst);
    o.sh.key_gate.open();
    settle(&mut o.s);
    assert_eq!(o.s.app.flash.as_deref(), Some("the message"));
    // The job drew once itself (dropped as stale); the redraw was asked for again.
    assert_eq!(o.sh.views() - views, 2, "the redraw was not lost");
    o.s.see("keys=2");
}

#[test]
fn e2e_canvas_a_huge_redraw_after_does_not_panic_and_is_bounded() {
    for d in [
        Duration::MAX,
        Duration::from_secs(u64::MAX),
        Duration::from_secs(1 << 40),
    ] {
        let mut o = open_with("cj_huge", "en", false, |sh| {
            sh.want_input.store(false, Ordering::SeqCst);
            *sh.after.lock().unwrap() = Some(d);
        });
        o.s.key('x');
        let at = o.s.app.canvas_redraw_deadline().expect("armed");
        assert!(
            at <= Instant::now() + crate::app::MAX_REDRAW_AFTER + Duration::from_secs(5),
            "bounded: {d:?}"
        );
        assert!(at >= Instant::now(), "{d:?}");
    }
}

#[test]
fn e2e_canvas_input_the_queue_of_keys_is_bounded_and_keeps_its_order() {
    let mut o = open_with("cj_cap", "en", true, |_| {});
    o.sh.hold_keys.store(true, Ordering::SeqCst);
    o.s.key('a'); // goes out at once and is held
                  // The last place is kept for `Esc`: the others take this many.
    let cap = crate::app::MAX_QUEUED_KEYS - 1;
    for i in 0..cap + 20 {
        o.s.key(char::from(b'a' + (i % 26) as u8));
    }
    assert_eq!(o.s.app.plugin_key_queue_len_for_test(), cap);
    assert!(o.s.app.flash.is_some(), "the refusal is told");
    o.sh.hold_keys.store(false, Ordering::SeqCst);
    o.sh.latch.open();
    settle(&mut o.s);
    let log = o.sh.log();
    assert_eq!(log.len(), 1 + cap);
    // The oldest are kept, in order; the refused ones are the newest.
    let want: String = std::iter::once('a')
        .chain((0..cap).map(|i| char::from(b'a' + (i % 26) as u8)))
        .collect();
    assert_eq!(chars(&log), want);
}

#[test]
fn e2e_canvas_input_esc_is_the_hosts_even_when_the_queue_is_full() {
    let mut o = open_with("cj_cap_esc", "en", true, |_| {});
    o.sh.hold_keys.store(true, Ordering::SeqCst);
    o.s.key('a');
    for _ in 0..crate::app::MAX_QUEUED_KEYS + 5 {
        o.s.key('b');
    }
    o.s.esc();
    assert!(!o.s.app.is_plugin_input(), "the user can always leave");
    o.sh.hold_keys.store(false, Ordering::SeqCst);
    o.sh.latch.open();
    settle(&mut o.s);
}

#[test]
fn e2e_canvas_input_a_long_paste_is_cut_at_a_character_boundary() {
    let mut o = open("cj_paste_cut");
    let max = crate::app::MAX_PASTE_BYTES;
    // Two-byte characters: the cut at an odd limit would split one.
    let text: String = "é".repeat(max); // 2 * max bytes
    o.s.app.handle_paste(text);
    let log = o.sh.log();
    assert_eq!(log.len(), 1);
    let PK::Paste(got) = &log[0].code else {
        panic!("a paste: {log:?}")
    };
    assert!(got.len() <= max && got.len() >= max - 1, "{}", got.len());
    assert!(got.chars().all(|c| c == 'é'));
    assert!(o.s.app.flash.is_some(), "the cut is told");
    // A paste within the limit is whole and silent.
    let mut o = open("cj_paste_ok");
    o.s.app.handle_paste("short".to_string());
    assert_eq!(o.sh.log(), vec![Key::paste("short")]);
    assert!(o.s.app.flash.is_none());
}

#[test]
fn e2e_canvas_an_answer_bigger_than_the_request_is_clipped_not_refused() {
    // A resize can race a plugin's answer: the canvas is drawn clipped to the area. It stays
    // within the `check()` limits (4096 a side, 4M cells), which are the cap.
    let mut o = open_with("cj_oversize", "en", false, |sh| {
        sh.want_input.store(false, Ordering::SeqCst);
        sh.oversize.store(true, Ordering::SeqCst);
    });
    o.s.key('x');
    o.s.draw();
    assert!(o.s.app.is_canvas_preview());
    assert!(o.s.app.plugin_failure().is_none());
    o.s.see("sess=1");
}

// ---- survivors of the stage-2.5 mutation audit (axis 4), batch E ---------------------------------

/// Put a batch of keys together in one job: `a` goes out at once and is held in `key`; the keys
/// typed meanwhile (`rest`) wait, and go together when `a` has been answered. `script` answers
/// the keys in order, `a` first.
fn one_batch(name: &str, script: Vec<Reply>, rest: &str) -> Opened {
    let mut o = open_with(name, "en", true, |sh| {
        *sh.script.lock().unwrap() = script.into();
    });
    o.sh.hold_keys.store(true, Ordering::SeqCst);
    o.s.key('a');
    for c in rest.chars() {
        o.s.key(c);
    }
    assert_eq!(
        o.s.app.plugin_key_queue_len_for_test(),
        rest.chars().count()
    );
    o.sh.hold_keys.store(false, Ordering::SeqCst);
    o.sh.latch.open();
    settle(&mut o.s);
    assert_eq!(chars(&o.sh.log()), format!("a{rest}"), "every key arrived");
    o
}

#[test]
fn e2e_canvas_a_batch_that_asks_for_a_reload_and_a_redraw_reads_the_file_again() {
    // `b` answers Reload and `c` Redraw in one job: the reload wins (the redraw would draw the
    // session that is going), in either order.
    for (i, pair) in [
        [Reply::Reload, Reply::Redraw],
        [Reply::Redraw, Reply::Reload],
    ]
    .into_iter()
    .enumerate()
    {
        let script = vec![Reply::Unhandled, pair[0].clone(), pair[1].clone()];
        let o = one_batch(&format!("cb_reload_{i}"), script, "bc");
        assert_eq!(
            o.sh.opens.load(Ordering::SeqCst),
            2,
            "order {i}: the file was read again"
        );
        assert_eq!(o.sh.drops(), 1, "order {i}: and the old session is gone");
        o.s.see("sess=2");
    }
}

#[test]
fn e2e_canvas_a_batch_that_redraws_without_a_reload_takes_one_new_view_on_the_same_session() {
    let o = one_batch(
        "cb_redraw",
        vec![Reply::Unhandled, Reply::Redraw, Reply::Redraw],
        "bc",
    );
    assert_eq!(o.sh.opens.load(Ordering::SeqCst), 1, "no reload");
    o.s.see("sess=1");
    o.s.see("keys=3");
}

#[test]
fn e2e_canvas_a_batch_that_asks_for_a_redraw_and_says_a_message_has_both() {
    // The message is shown and the redraw is not lost, whatever the order of the two answers.
    for (i, pair) in [
        [Reply::Redraw, Reply::Flash("the batch said")],
        [Reply::Flash("the batch said"), Reply::Redraw],
    ]
    .into_iter()
    .enumerate()
    {
        let script = vec![Reply::Unhandled, pair[0].clone(), pair[1].clone()];
        let o = one_batch(&format!("cb_flash_{i}"), script, "bc");
        assert_eq!(
            o.s.app.flash.as_deref(),
            Some("the batch said"),
            "order {i}: the message is shown"
        );
        o.s.see("keys=3");
        assert_eq!(o.sh.opens.load(Ordering::SeqCst), 1, "order {i}: no reload");
    }
}

#[test]
fn e2e_canvas_a_batch_with_a_message_and_no_redraw_says_it_and_takes_no_view() {
    let mut o = one_batch(
        "cb_flash_only",
        vec![
            Reply::Unhandled,
            Reply::Flash("only a message"),
            Reply::Unhandled,
        ],
        "bc",
    );
    assert_eq!(o.s.app.flash.as_deref(), Some("only a message"));
    let views = o.sh.views();
    o.s.draw();
    assert_eq!(o.sh.views(), views, "nobody asked for a view");
}

// ---- a refit asks again when only the height changed -----------------------------------------------

#[test]
fn e2e_canvas_a_resize_of_the_height_alone_asks_the_plugin_again() {
    let mut o = open("cr_height");
    let (cols, rows) = *o.sh.sizes().last().expect("asked");
    let views = o.sh.views();
    // Same width, 6 rows less.
    o.s.resize(90, 20);
    o.s.draw();
    assert!(o.sh.views() > views, "the plugin was asked again");
    let (c2, r2) = *o.sh.sizes().last().unwrap();
    assert_eq!(c2, cols, "the width is the same");
    assert_eq!(r2, rows - 6, "and the rows follow the terminal");
    // Nothing more is asked while the size stays.
    let views = o.sh.views();
    o.s.draw();
    o.s.draw();
    assert_eq!(o.sh.views(), views, "once per size");
    // The other direction too.
    o.s.resize(90, 30);
    o.s.draw();
    assert_eq!(o.sh.sizes().last().unwrap().1, r2 + 10);
}

#[test]
fn e2e_canvas_a_resize_of_the_width_alone_asks_the_plugin_again() {
    let mut o = open("cr_width");
    let (cols, rows) = *o.sh.sizes().last().expect("asked");
    let views = o.sh.views();
    o.s.resize(70, 26);
    o.s.draw();
    assert!(o.sh.views() > views);
    let (c2, r2) = *o.sh.sizes().last().unwrap();
    assert_eq!((c2, r2), (cols - 20, rows));
}

// ---- the Esc latch survives a failed view and a reading again ----------------------------------------

#[test]
fn e2e_canvas_input_a_failed_view_drops_the_deadline_and_the_file_read_again_starts_clean() {
    let mut o = open_with("ce_esc_fail", "en", false, |sh| {
        *sh.after.lock().unwrap() = Some(AFTER);
    });
    o.s.esc();
    assert!(!o.s.app.is_plugin_input());
    assert!(
        o.s.app.canvas_redraw_deadline().is_some(),
        "the canvas on screen wants the timer"
    );
    // The user presses the plugin's action and the view it asks for fails.
    o.sh.fail_views.store(true, Ordering::SeqCst);
    o.s.key('x');
    assert!(o.s.app.plugin_failure().is_some(), "the view failed");
    assert!(!o.s.app.is_canvas_preview());
    assert_eq!(
        o.s.app.canvas_redraw_deadline(),
        None,
        "a failed show has no timer"
    );
    assert!(!o.s.app.is_plugin_input(), "no canvas, no typing");
    // The file is read again: that is a new show, and a show starts clean (like opening the file
    // again), so the canvas that asks for the typing takes it.
    o.sh.fail_views.store(false, Ordering::SeqCst);
    touch_file(&o);
    o.s.app.reload_preview();
    o.s.draw();
    assert!(o.s.app.is_canvas_preview(), "the canvas is back");
    assert!(o.s.app.is_plugin_input(), "a new show starts clean");
    assert!(o.s.app.canvas_redraw_deadline().is_some());
}

#[test]
fn e2e_canvas_input_a_failure_the_users_action_caused_does_not_release_the_latch() {
    // The failure of the view an action asked for is "the user's" answer, and it is not a canvas:
    // it must not give the typing back. A canvas that comes after it, when nobody asked, does not
    // either (the latch is the user's to release).
    let mut o = open("ce_esc_user_fail");
    o.s.esc();
    assert!(!o.s.app.is_plugin_input());
    o.s.app.canvas_landed_for_test(Some(None), true);
    assert!(!o.s.app.is_plugin_input());
    o.s.app
        .canvas_landed_for_test(Some(Some((true, None))), false);
    assert!(
        !o.s.app.is_plugin_input(),
        "a canvas nobody asked for does not take the typing after a failure"
    );
    // And a view that is not a canvas, the user's, leaves it as it was too.
    o.s.app.canvas_landed_for_test(Some(None), true);
    o.s.app
        .canvas_landed_for_test(Some(Some((true, None))), false);
    assert!(!o.s.app.is_plugin_input());
    // The user's request releases it.
    o.s.app
        .canvas_landed_for_test(Some(Some((true, None))), true);
    assert!(o.s.app.is_plugin_input());
}

// ---- the re-arm of a skipped deadline is kept within the bounds ---------------------------------------

#[test]
fn e2e_canvas_timer_a_deadline_armed_again_by_a_result_without_a_view_is_within_the_bounds() {
    for (i, ask) in [
        Duration::ZERO,
        Duration::from_millis(1),
        Duration::from_millis(32),
        Duration::from_millis(50),
        Duration::from_secs(10),
        Duration::MAX,
    ]
    .into_iter()
    .enumerate()
    {
        let mut o = open_with(&format!("ct_rearm_{i}"), "en", false, |sh| {
            *sh.after.lock().unwrap() = Some(ask);
            *sh.reply.lock().unwrap() = Reply::Unhandled;
        });
        // The tick passed while a job was out, so no deadline is waiting.
        o.s.app.set_canvas_redraw_at_for_test(None);
        let before = Instant::now();
        // A key that changes nothing: the result carries no view, and the canvas on screen is armed
        // from its own `redraw_after`.
        o.s.key('b');
        let after = Instant::now();
        let at = o.s.app.canvas_redraw_deadline().expect("armed again");
        let kept = ask.clamp(
            Duration::from_millis(33), // the documented lower bound
            crate::app::MAX_REDRAW_AFTER,
        );
        assert!(at >= before + kept, "{ask:?}: not earlier than {kept:?}");
        assert!(at <= after + kept, "{ask:?}: not later than {kept:?}");
    }
}

// ---- a menu opened before the canvas asked for the typing does not swallow the typing ----------------

#[test]
fn e2e_canvas_input_a_key_typed_in_the_capture_closes_a_menu_opened_before() {
    let mut o = open_with("cl_leader_key", "en", false, |sh| {
        sh.want_input.store(false, Ordering::SeqCst);
    });
    o.s.key('y');
    assert_eq!(o.s.app.pending_leader, Some(crate::keymap::LeaderId::Copy));
    // The canvas now asks for the typing (the answer to a tick), with the menu still open.
    o.sh.want_input.store(true, Ordering::SeqCst);
    tick(&mut o);
    assert!(o.s.app.is_plugin_input(), "the capture began");
    assert_eq!(
        o.s.app.pending_leader,
        Some(crate::keymap::LeaderId::Copy),
        "nothing closed the menu yet"
    );
    o.s.key('z');
    assert_eq!(chars(&o.sh.log()), "z", "the key is the plugin's");
    assert_eq!(o.s.app.pending_leader, None, "and the menu is closed");
}

#[test]
fn e2e_canvas_input_a_paste_in_the_capture_closes_a_menu_opened_before() {
    let mut o = open_with("cl_leader_paste", "en", false, |sh| {
        sh.want_input.store(false, Ordering::SeqCst);
    });
    o.s.key('y');
    assert_eq!(o.s.app.pending_leader, Some(crate::keymap::LeaderId::Copy));
    o.sh.want_input.store(true, Ordering::SeqCst);
    tick(&mut o);
    assert!(o.s.app.is_plugin_input());
    assert_eq!(o.s.app.pending_leader, Some(crate::keymap::LeaderId::Copy));
    o.s.app.handle_paste("pasted".to_string());
    assert_eq!(o.sh.log(), vec![Key::paste("pasted")]);
    assert_eq!(o.s.app.pending_leader, None, "the menu is closed");
}

// ---- the UI thread never waits for a keys job ---------------------------------------------------------

#[test]
fn e2e_canvas_input_a_slow_key_does_not_make_the_ui_thread_wait() {
    // There is no seam for the wait the handoff is given (`Wait::Result(ZERO)`), so this watches
    // the effect: with a key held in the plugin, the key press returns at once, with the session
    // out. A wait of seconds (the budget of a first frame is 30 ms; a mutation to 5 s) makes the
    // press last that long, because the worker is held by the gate and cannot answer.
    let mut o = open_with("cw_zero", "en", true, |_| {});
    o.sh.hold_key_gate.store(true, Ordering::SeqCst);
    let started = Instant::now();
    o.s.key('a');
    let took = started.elapsed();
    assert!(
        took < Duration::from_secs(2),
        "the UI thread waited {took:?} for a held key"
    );
    assert!(o.s.app.plugin_session_is_out_for_test(), "the job is out");
    assert!(o.sh.log().is_empty(), "and the plugin has not answered");
    // The same for a key queued behind it, and a paste.
    let started = Instant::now();
    o.s.key('b');
    o.s.app.handle_paste("p".into());
    assert!(started.elapsed() < Duration::from_secs(2));
    o.sh.hold_key_gate.store(false, Ordering::SeqCst);
    o.sh.key_gate.open();
    settle(&mut o.s);
    assert_eq!(o.sh.log().len(), 3, "every key arrived");
}

// ---- what the host lists for a canvas: footer, `?` help, menus ---------------------------------------
//
// A canvas keeps a handful of host keys (`keys::reserved("canvas")`): the page keys, the outline
// (page list), the representation, the file and tab jumps, the bookmarks, `r`, `Y`, the verbs, `?`,
// `q`. The footer, the help and the menus list each of them while it acts. These read the plans
// of the canvas context directly (`Context::Canvas`), with a session that has pages and two
// representations so that the keys that need them act.

fn paged(name: &str, lang: &str) -> Opened {
    open_with(name, lang, false, |sh| {
        sh.pages.store(true, Ordering::SeqCst);
        sh.reprs.store(true, Ordering::SeqCst);
        sh.want_input.store(false, Ordering::SeqCst);
    })
}

fn canvas_tokens(o: &Opened) -> Vec<String> {
    crate::ops::footer::tokens(&o.s.app, crate::keymap::Context::Canvas)
}

#[test]
fn e2e_canvas_the_footer_lists_every_host_key_of_a_canvas_that_acts() {
    let o = paged("co_footer", "en");
    let tokens = canvas_tokens(&o);
    // The host's keys come in this order, the plugin's own right after `q:back`.
    let expected = [
        "q:back",
        "x:refresh",
        "z:second",
        "J/K:page",
        "O:pages",
        "R:Two",
        "C-n/C-p:next/prev file",
        "?:help",
        "e:edit",
        "o:open in app",
        "[/]:tab",
        ",:view",
    ];
    assert_eq!(tokens, expected, "{tokens:?}");
}

#[test]
fn e2e_canvas_the_footer_without_pages_and_representations_lists_the_rest() {
    // The page keys, the page list and `R` show only while they act.
    let o = open_with("co_footer_plain", "en", false, |sh| {
        sh.want_input.store(false, Ordering::SeqCst);
    });
    let tokens = canvas_tokens(&o);
    for t in ["J/K:page", "O:pages", "R:Two"] {
        assert!(!tokens.iter().any(|x| x == t), "{t} in {tokens:?}");
    }
    for t in ["q:back", "?:help", "[/]:tab", ",:view", "e:edit"] {
        assert!(tokens.iter().any(|x| x == t), "{t} missing from {tokens:?}");
    }
}

#[test]
fn e2e_canvas_the_footer_is_in_japanese_when_the_ui_is() {
    let o = paged("co_footer_ja", "ja");
    let tokens = canvas_tokens(&o);
    let en = canvas_tokens(&paged("co_footer_ja_en", "en"));
    assert_eq!(tokens.len(), en.len());
    assert_ne!(tokens, en, "the labels follow the language");
    assert!(tokens.iter().any(|t| t.starts_with("?:")), "{tokens:?}");
}

#[test]
fn e2e_canvas_the_help_lists_the_host_keys_under_the_canvas_title() {
    let o = paged("co_help", "en");
    let sections = crate::ops::help::sections_for(&o.s.app, crate::keymap::Context::Canvas);
    let own = sections
        .iter()
        .find(|s| s.title == "Preview: plugin view")
        .unwrap_or_else(|| {
            panic!(
                "sections: {:?}",
                sections.iter().map(|s| &s.title).collect::<Vec<_>>()
            )
        });
    let rows: Vec<(&str, &str)> = own
        .rows
        .iter()
        .map(|(k, d)| (k.as_str(), d.as_str()))
        .collect();
    let has = |key: &str, desc_part: &str| {
        assert!(
            rows.iter().any(|(k, d)| *k == key && d.contains(desc_part)),
            "no row `{key}` / `{desc_part}` in {rows:?}"
        );
    };
    has("J / K", "page");
    has("O", "pages");
    has("R", "Two");
    has("Ctrl-n / Ctrl-p", "next / previous file");
    has("m / '", "bookmark");
    has("r", "refresh");
    has("Y", "@path");
    has("q / Esc", "back to tree");
    // Nothing that would scroll or search is promised.
    for scrolling in ["j / k", "g / G", "h / j / k / l", "/  n / N", "/", "v / V"] {
        assert!(
            !rows.iter().any(|(k, _)| *k == scrolling),
            "{scrolling} in {rows:?}"
        );
    }
    // The plugin's own keys are in a section of their own, under its name.
    let plugin = sections
        .iter()
        .find(|s| s.title == "co_help")
        .expect("plugin section");
    assert!(plugin
        .rows
        .iter()
        .any(|(k, d)| k == "x" && d == "Draw again"));
}

#[test]
fn e2e_canvas_the_help_edit_row_says_the_external_editor_not_the_environment_one() {
    // In a canvas (as in a picture or a table) `e` opens the file in the external editor; in a text
    // preview the row names the environment variable the editor comes from.
    let o = paged("co_help_edit", "en");
    let launch = |ctx| {
        crate::ops::help::rows_for(&o.s.app, ctx)
            .into_iter()
            .find(|r| r.actions.contains(&crate::keymap::Action::RequestEdit))
            .map(|r| r.desc)
            .expect("an `e` row")
    };
    let canvas = launch(crate::keymap::Context::Canvas);
    assert_eq!(canvas, launch(crate::keymap::Context::Image));
    assert_eq!(canvas, launch(crate::keymap::Context::Table));
    assert_ne!(
        canvas,
        launch(crate::keymap::Context::Text),
        "the text preview's row is the other one"
    );
    assert_eq!(canvas, "edit in external editor");
}

#[test]
fn e2e_canvas_the_menus_offer_the_path_style_and_the_host_keys() {
    use crate::keymap::{Action, Context, LeaderId};
    use crate::ops::menu::{menu, View};
    let o = paged("co_menu", "en");
    // `,` : the path style, which has no key of its own in a canvas.
    let options = menu(&o.s.app, Context::Canvas, LeaderId::Options, View::Open).expect("a menu");
    assert_eq!(
        options
            .items
            .iter()
            .map(|i| (i.key.label(), i.action))
            .collect::<Vec<_>>(),
        vec![("p".to_string(), Action::CyclePathStyle)]
    );
    // `Space`: the plugin's actions and the host's, the path style among them.
    let actions = menu(&o.s.app, Context::Canvas, LeaderId::Actions, View::Open).expect("a menu");
    let has = |a: Action| actions.items.iter().any(|i| i.action == a);
    for a in [
        Action::PreviewBack,
        Action::PageNext,
        Action::PagePrev,
        Action::ToggleOutline,
        Action::CycleRepresentation,
        Action::PreviewFileNext,
        Action::PreviewFilePrev,
        Action::ToggleHelp,
        Action::RequestEdit,
        Action::OpenExternal,
        Action::TabPrev,
        Action::TabNext,
        Action::Refresh,
        Action::CyclePathStyle,
    ] {
        assert!(has(a), "{a:?} missing from {:?}", actions.items);
    }
}

#[test]
fn e2e_canvas_the_canvas_context_writes_enter_short_and_no_navigation_key_is_host_bound() {
    use crate::keymap::Context;
    assert!(crate::ops::keys::short_enter(Context::Canvas));
    // The fixed keys of a canvas: `Esc` only.
    let fixed = crate::ops::keys::fixed_bindings(Context::Canvas);
    assert_eq!(fixed.len(), 1);
    assert_eq!(fixed[0].action, crate::keymap::Action::Cancel);
}

/// A canvas that reports pages is a canvas, not a table: its footer, its `?` help and its keys
/// are the canvas's (a table binds `h/j/k/l`, `Enter` and `/` itself).
#[test]
fn e2e_canvas_a_canvas_with_pages_is_still_a_canvas_context() {
    let o = paged("co_paged_ctx", "en");
    assert!(o.s.app.is_canvas_preview());
    assert_eq!(
        o.s.app.key_context(),
        Some(crate::keymap::Context::Canvas),
        "footer: {}",
        footer(&o.s)
    );
    assert!(!o.s.app.is_table_preview());
}

fn paged_with_moves(name: &str) -> Opened {
    open_with(name, "en", false, |sh| {
        sh.pages.store(true, Ordering::SeqCst);
        sh.moves.store(true, Ordering::SeqCst);
        sh.want_input.store(false, Ordering::SeqCst);
    })
}

#[test]
fn e2e_canvas_a_paged_canvas_runs_the_plugins_actions_on_j_and_enter() {
    let mut o = paged_with_moves("co_paged_moves");
    assert!(o.s.app.is_canvas_preview());
    assert!(!o.s.app.is_table_preview(), "a paged canvas is no table");
    assert_eq!(o.s.app.surface(), Surface::PreviewCanvas);
    assert_eq!(o.s.app.key_context(), Some(crate::keymap::Context::Canvas));
    o.s.key('j');
    o.s.enter();
    assert_eq!(
        o.sh.acts(),
        ["down", "pick"],
        "both keys ran the plugin's action"
    );
    // The page keys are still the host's, on the same surface.
    o.s.key('J');
    assert!(o.s.app.is_canvas_preview());
    assert_eq!(o.s.app.key_context(), Some(crate::keymap::Context::Canvas));
}

#[test]
fn e2e_canvas_a_paged_canvas_shows_the_canvas_footer_help_and_no_table_chip() {
    let o = paged_with_moves("co_paged_help");
    let foot = footer(&o.s);
    assert!(foot.contains("J/K:page"), "{foot}");
    assert!(!foot.contains("cell"), "no table hints: {foot}");
    assert_eq!(
        crate::ops::help::help_context(&o.s.app),
        crate::keymap::Context::Canvas
    );
    assert!(!o.s.app.is_table_preview());
    assert!(o.s.app.page_indicator().is_some(), "the page chip is shown");
}

// ---- batch F: the paste backlog, the way out, the follow-up ask --------------------------------------

#[test]
fn e2e_canvas_input_esc_is_delivered_to_the_plugin_even_when_the_queue_is_full() {
    let mut o = open_with("cj_esc_delivered", "en", true, |_| {});
    o.sh.hold_keys.store(true, Ordering::SeqCst);
    o.s.key('a'); // out, held
    for _ in 0..crate::app::MAX_QUEUED_KEYS + 5 {
        o.s.key('b');
    }
    assert_eq!(
        o.s.app.plugin_key_queue_len_for_test(),
        crate::app::MAX_QUEUED_KEYS - 1,
        "the last place is kept"
    );
    o.s.esc();
    assert!(!o.s.app.is_plugin_input(), "the user left");
    assert_eq!(
        o.s.app.plugin_key_queue_len_for_test(),
        crate::app::MAX_QUEUED_KEYS,
        "and the plugin is told: Esc took the reserved place"
    );
    o.sh.hold_keys.store(false, Ordering::SeqCst);
    o.sh.latch.open();
    settle(&mut o.s);
    let log = o.sh.log();
    assert_eq!(
        log.last().map(|k| &k.code),
        Some(&PK::Esc),
        "Esc was delivered"
    );
    assert_eq!(log.len(), 1 + crate::app::MAX_QUEUED_KEYS);
}

#[test]
fn e2e_canvas_input_the_pastes_waiting_for_the_session_are_bounded_in_total() {
    let mut o = open_with("cj_paste_total", "en", true, |_| {});
    o.sh.hold_keys.store(true, Ordering::SeqCst);
    o.s.key('a'); // out, held
    let big = "x".repeat(crate::app::MAX_PASTE_BYTES);
    for _ in 0..6 {
        o.s.app.plugin_input_paste(big.clone());
    }
    assert_eq!(
        o.s.app.plugin_queued_paste_bytes_for_test(),
        crate::app::MAX_QUEUED_PASTE_BYTES,
        "4 MiB of 6 MiB"
    );
    assert!(o.s.app.flash.is_some(), "the refusal is told");
    // A smaller paste is cut to what is left, never reordered or dropped silently.
    o.sh.hold_keys.store(false, Ordering::SeqCst);
    o.sh.latch.open();
    settle(&mut o.s);
    let log = o.sh.log();
    let pastes = log
        .iter()
        .filter(|k| matches!(k.code, PK::Paste(_)))
        .count();
    assert_eq!(pastes, 4);
}

#[test]
fn e2e_canvas_input_a_paste_is_cut_to_what_is_left_of_the_total() {
    let mut o = open_with("cj_paste_left", "en", true, |_| {});
    o.sh.hold_keys.store(true, Ordering::SeqCst);
    o.s.key('a');
    let max = crate::app::MAX_PASTE_BYTES;
    for _ in 0..3 {
        o.s.app.plugin_input_paste("y".repeat(max));
    }
    // 3 MiB are waiting; a 1 MiB + 10 paste is first cut to 1 MiB (its own limit) and fits.
    o.s.app.plugin_input_paste("z".repeat(max + 10));
    assert_eq!(
        o.s.app.plugin_queued_paste_bytes_for_test(),
        4 * max,
        "cut at its own limit"
    );
    // Now 1 MiB is left in no case: the next paste is refused whole.
    o.s.app.flash = None;
    o.s.app.plugin_input_paste("w".to_string());
    assert!(o.s.app.flash.is_some());
    assert_eq!(o.s.app.plugin_queued_paste_bytes_for_test(), 4 * max);
    o.sh.hold_keys.store(false, Ordering::SeqCst);
    o.sh.latch.open();
    settle(&mut o.s);
}

#[test]
fn e2e_canvas_the_follow_up_ask_is_at_most_one_per_33_ms() {
    let mut o = open_with("co_more_rate", "en", false, |sh| {
        sh.want_input.store(false, Ordering::SeqCst);
    });
    let t0 = Instant::now();
    let floor = crate::app::MIN_REDRAW_AFTER;
    assert!(!o.s.app.plugin_wants_view_for_test());
    // The first ask goes at once.
    o.s.app.canvas_ask_more_for_test(t0);
    assert!(o.s.app.plugin_wants_view_for_test());
    o.s.app.clear_plugin_wants_view_for_test();
    // One within the floor does not: the redraw timer is set to the first moment it may go.
    o.s.app.canvas_ask_more_for_test(t0 + floor / 3);
    assert!(!o.s.app.plugin_wants_view_for_test());
    assert_eq!(o.s.app.canvas_redraw_deadline(), Some(t0 + floor));
    // After the floor it goes again.
    o.s.app
        .canvas_ask_more_for_test(t0 + floor + Duration::from_millis(1));
    assert!(o.s.app.plugin_wants_view_for_test());
}

#[test]
fn e2e_canvas_the_follow_up_ask_waits_while_the_canvas_is_covered() {
    let mut o = open_with("co_more_covered", "en", false, |sh| {
        sh.want_input.store(false, Ordering::SeqCst);
    });
    o.s.key('?');
    assert!(o.s.app.show_help);
    let t0 = Instant::now();
    o.s.app.canvas_ask_more_for_test(t0);
    assert!(
        !o.s.app.plugin_wants_view_for_test(),
        "the plugin is not asked for a canvas nobody sees"
    );
    assert_eq!(
        o.s.app.canvas_redraw_deadline(),
        Some(t0),
        "the timer asks when it is seen"
    );
}

#[test]
fn e2e_canvas_the_follow_up_floor_counts_from_the_landing_of_the_previous_answer() {
    let mut o = open_with("co_more_landing", "en", false, |sh| {
        sh.want_input.store(false, Ordering::SeqCst);
    });
    let t0 = Instant::now();
    let floor = crate::app::MIN_REDRAW_AFTER;
    // The answer to the user's own action: no ask came before it, the follow-up goes at once.
    o.s.app.canvas_ask_more_after_landing(t0);
    assert!(o.s.app.plugin_wants_view_for_test());
    o.s.app.clear_plugin_wants_view_for_test();
    // That ask was answered 10 floors later (the job took long): the answer's own follow-up waits
    // one floor from the landing, not from the ask (which was long ago).
    let landed = t0 + floor * 10;
    o.s.app.canvas_ask_more_after_landing(landed);
    assert!(
        !o.s.app.plugin_wants_view_for_test(),
        "a job longer than the floor does not make the next ask due at once"
    );
    assert_eq!(o.s.app.canvas_redraw_deadline(), Some(landed + floor));
    // The timer asks at that deadline, and an answer landing right after waits again.
    assert!(o.s.app.canvas_redraw_if_due(landed + floor));
}
