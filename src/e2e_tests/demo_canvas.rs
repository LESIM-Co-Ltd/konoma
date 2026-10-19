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
    s.see("press i to type");
}

#[test]
fn e2e_demo_canvas_typing_enter_esc_and_clear() {
    let (mut s, _dir) = open_demo("demo_canvas_type");
    assert!(!s.app.is_plugin_input());
    s.key('i');
    assert!(s.app.is_plugin_input(), "`i` starts the line editor");
    s.keys("hello");
    s.see("> hello_");
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
