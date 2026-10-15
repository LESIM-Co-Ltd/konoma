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
        })
    }

    fn log(&self) -> Vec<Key> {
        self.log.lock().unwrap().clone()
    }

    fn views(&self) -> usize {
        self.views.load(Ordering::SeqCst)
    }
}

struct Typist {
    id: String,
    sh: Arc<Shared>,
}

impl Plugin for Typist {
    fn manifest(&self) -> Manifest {
        Manifest::new(self.id.as_str(), "0.1.0")
            .bind(Bind::files(["*.typ"]))
            .shows(ViewKind::Canvas)
            .action(
                Act::new("refresh")
                    .key("x")
                    .label(t!(en: "refresh", ja: "更新"))
                    .help(t!(en: "Draw again", ja: "描き直す"))
                    .footer(1),
            )
    }

    fn open(&self, _: Input, _: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        self.sh.opens.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(TypistSession {
            sh: self.sh.clone(),
        }))
    }
}

struct TypistSession {
    sh: Arc<Shared>,
}

impl Session for TypistSession {
    fn view(&mut self, req: &ViewReq, _: &ViewCx) -> Result<View, Failure> {
        self.sh.views.fetch_add(1, Ordering::SeqCst);
        if self.sh.hold_views.load(Ordering::SeqCst) {
            self.sh.latch.wait();
        }
        let mut c = CanvasView::new(req.cols, req.rows);
        c.set_string(
            0,
            0,
            &format!("keys={}", self.sh.log.lock().unwrap().len()),
            PStyle::new(),
        );
        c.text_input = self.sh.want_input.load(Ordering::SeqCst);
        c.redraw_after = *self.sh.after.lock().unwrap();
        Ok(View::Canvas(c))
    }

    fn act(&mut self, action: &str, _: &ActCx) -> Result<Outcome, Failure> {
        match action {
            "refresh" => Ok(Outcome::Redraw),
            _ => Ok(Outcome::Unhandled),
        }
    }

    fn key(&mut self, key: &Key, _: &ActCx) -> Result<Outcome, Failure> {
        if self.sh.hold_keys.load(Ordering::SeqCst) {
            self.sh.latch.wait();
        }
        self.sh.log.lock().unwrap().push(key.clone());
        match self.sh.reply.lock().unwrap().clone() {
            Reply::Redraw => Ok(Outcome::Redraw),
            Reply::Unhandled => Ok(Outcome::Unhandled),
            Reply::Flash(m) => Ok(Outcome::Flash(Tr::new(m).ja(format!("{m} (ja)")))),
            Reply::Reload => Ok(Outcome::Reload),
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

#[test]
fn e2e_canvas_input_is_not_taken_again_until_the_plugin_says_false_then_true() {
    let mut o = open("ci_notrap");
    o.s.esc();
    assert!(!o.s.app.is_plugin_input());
    // The plugin answered Redraw to Esc and still asks for the typing: the host does not take it.
    o.s.key('?');
    assert!(o.s.app.show_help, "the host's key, not text");
    o.s.key('q');
    assert!(!o.s.app.show_help);
    assert!(o.s.app.is_canvas_preview());
    // More canvases that still ask: still not captured.
    o.s.key('x');
    assert!(!o.s.app.is_plugin_input());
    // The plugin shows a canvas that does not ask...
    o.sh.want_input.store(false, Ordering::SeqCst);
    o.s.key('x');
    assert!(!o.s.app.is_plugin_input());
    // ...and later one that does: a new request, taken.
    o.sh.want_input.store(true, Ordering::SeqCst);
    o.s.key('x');
    assert!(
        o.s.app.is_plugin_input(),
        "false then true is a new request"
    );
    let before = o.sh.log().len();
    o.s.key('?');
    assert_eq!(o.sh.log().len(), before + 1, "and `?` is text again");
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
fn e2e_canvas_timer_runs_under_the_help_overlay_like_the_gif_timers_do() {
    // The condition is the GIF timers' (preview mode, this file): an overlay in front does not
    // stop the animation behind it.
    let mut o = ticking("ct_help");
    o.s.key('?');
    assert!(o.s.app.show_help);
    let at = o.s.app.canvas_redraw_deadline().unwrap();
    let views = o.sh.views();
    assert!(o.s.app.canvas_redraw_if_due(at));
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
