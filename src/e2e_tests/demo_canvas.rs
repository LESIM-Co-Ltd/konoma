//! The temporary `demo-canvas` plugin (feature `demo-canvas`) opened through the shipped registry:
//! `samples/demo.konoma-demo` is shown as a canvas, the line editor takes the typing, `Esc` leaves
//! it, and an action on an arrow key changes what is on the screen. Removed with the feature when
//! stage 3 can load external plugins.

use super::*;

fn open_demo(name: &str) -> (Sim, crate::test_support::TmpDir) {
    let dir = sandbox(name);
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("samples/demo.konoma-demo");
    std::fs::copy(src, dir.join("demo.konoma-demo")).unwrap();
    let mut s = Sim::new(&canon(&dir));
    s.select("demo.konoma-demo");
    s.enter();
    s.draw();
    (s, dir)
}

#[test]
fn e2e_demo_canvas_shows_the_title_the_gauge_and_the_hint() {
    let (s, _dir) = open_demo("demo_canvas_open");
    assert!(s.app.is_canvas_preview());
    s.see("konoma canvas demo");
    s.see("0 s");
    s.see("i: type a note");
}

#[test]
fn e2e_demo_canvas_typing_enter_esc_and_clear() {
    let (mut s, _dir) = open_demo("demo_canvas_type");
    assert!(!s.app.is_plugin_input());
    s.key('i');
    assert!(s.app.is_plugin_input(), "`i` starts the line editor");
    s.keys("hello");
    s.see("> hello█");
    s.enter();
    s.see("- hello");
    s.keys("x");
    s.esc();
    assert!(!s.app.is_plugin_input(), "Esc leaves the capture");
    assert!(s.app.is_canvas_preview());
    // Back with the host's keys: `?` is the help again, and `c` clears the notes.
    s.key('c');
    assert!(!s.screen().contains("- hello"), "`c` cleared the notes");
}

#[test]
fn e2e_demo_canvas_an_arrow_key_changes_the_gauge_colour() {
    let (mut s, _dir) = open_demo("demo_canvas_hue");
    s.see("green");
    s.press(KeyCode::Right, KeyModifiers::NONE);
    s.see("magenta");
    s.press(KeyCode::Right, KeyModifiers::NONE);
    s.see("yellow");
    s.press(KeyCode::Left, KeyModifiers::NONE);
    s.see("magenta");
    assert!(s.app.is_canvas_preview(), "the arrow stays in the plugin");
}

fn open_demo_ja(name: &str) -> (Sim, crate::test_support::TmpDir) {
    let dir = sandbox(name);
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("samples/demo.konoma-demo");
    std::fs::copy(src, dir.join("demo.konoma-demo")).unwrap();
    let mut cfg = crate::config::Config::default();
    cfg.ui.lang = "ja".into();
    let mut s = Sim::with_config(&canon(&dir), cfg);
    s.select("demo.konoma-demo");
    s.enter();
    s.draw();
    (s, dir)
}

/// The test terminal draws wide glyphs with a blank after each: compare without blanks.
fn squeezed(s: &Sim) -> String {
    s.screen().chars().filter(|c| !c.is_whitespace()).collect()
}

#[test]
fn e2e_demo_canvas_explains_itself_in_english() {
    let (mut s, _dir) = open_demo("demo_canvas_help_en");
    s.see("i: type a note");
    s.see("gauge colour");
    assert!(
        !s.screen().contains("c: clear notes"),
        "nothing to clear yet"
    );
    s.key('i');
    s.see("Typing");
    s.see("Esc leaves");
    s.keys("hi");
    s.enter();
    s.see("Notes");
    s.see("- hi");
    s.esc();
    s.see("c: clear notes");
    s.key('c');
    assert!(
        !s.screen().contains("Notes"),
        "the heading goes with the notes"
    );
}

#[test]
fn e2e_demo_canvas_explains_itself_in_japanese() {
    let (mut s, _dir) = open_demo_ja("demo_canvas_help_ja");
    assert!(squeezed(&s).contains("i:メモを入力"), "{}", s.screen());
    assert!(squeezed(&s).contains("ゲージの色"));
    assert!(!squeezed(&s).contains("メモを消す"));
    s.key('i');
    assert!(squeezed(&s).contains("入力中"), "{}", s.screen());
    assert!(squeezed(&s).contains("Escで抜ける"));
    s.keys("hello");
    s.see("> hello█");
    s.enter();
    assert!(squeezed(&s).contains("-hello"));
    s.esc();
    assert!(squeezed(&s).contains("c:メモを消す"), "{}", s.screen());
}
