//! `R` walks the representations any plugin declares (`Layout.reprs`), not just the standard
//! names: a plugin of every display type (a document, a table, a picture, text) under an id of its
//! own with three representations is cycled in the declared order, wraps after the last, keeps the
//! page, and names the representation the key goes to in the footer and in the `?` help. A plugin
//! with a single representation gets no hint and the key does nothing.

use super::*;
use crate::plugins::test_plugins;
use crate::plugins::test_views::{Multi, Shape};
use konoma_plugin::Plugin;
use std::sync::Arc;

fn cfg_for(name: &str, plugin: Arc<dyn Plugin>) -> Config {
    let mut cfg = Config::default();
    cfg.ui.lang = "en".into();
    let marker = test_plugins::register(name, plugin);
    let mut t = toml::Table::new();
    t.insert("enabled".into(), toml::Value::Boolean(false));
    cfg.plugins.insert(marker, t);
    cfg
}

fn open(
    name: &str,
    file: &str,
    shape: Shape,
    reprs: usize,
    pages: usize,
) -> (Sim, crate::test_support::TmpDir) {
    let dir = sandbox(name);
    std::fs::write(dir.join(file), b"x").unwrap();
    let ext = file.rsplit('.').next().unwrap();
    let ext: &'static str = Box::leak(ext.to_string().into_boxed_str());
    let id: &'static str = Box::leak(format!("x-{name}").into_boxed_str());
    let cfg = cfg_for(
        name,
        Arc::new(Multi {
            id,
            ext,
            shape,
            reprs,
            pages,
        }),
    );
    let mut s = Sim::with_config_sized(&canon(&dir), cfg, 100, 30);
    // Only a picture needs the media worker; the other displays land on the spot.
    if matches!(shape, Shape::Image) {
        s = s.with_media();
    }
    s.select(file);
    s.enter();
    settle(&mut s);
    (s, dir)
}

fn r_hint(s: &Sim) -> Option<String> {
    crate::ui::preview::footer_hints(&s.app)
        .into_iter()
        .find(|h| h.starts_with("R:"))
}

fn red(s: &Sim) -> (u8, u8) {
    let px = s.app.shown_picture_rgba().expect("a picture").2;
    (px[0], px[1])
}

/// A picture is drawn on the media worker: wait for it (the other displays land on the spot).
fn settle(s: &mut Sim) {
    if s.app.shows_media_plugin() {
        s.drain_media();
    }
    s.draw();
}

/// Press `R`, let what it asked for land.
fn r(s: &mut Sim) {
    s.key('R');
    settle(s);
}

#[test]
fn r_walks_the_three_representations_of_a_document_and_keeps_the_slide() {
    let (mut s, _d) = open("rc_doc", "a.xdoc", Shape::Doc, 3, 3);
    s.see("this is alpha");
    assert_eq!(r_hint(&s).as_deref(), Some("R:Beta"), "names the next one");
    s.key('J');
    assert_eq!(s.app.slide_position(), Some((2, 3)));
    r(&mut s);
    s.see("this is beta");
    assert_eq!(r_hint(&s).as_deref(), Some("R:Gamma"));
    assert_eq!(s.app.slide_position(), Some((2, 3)), "the slide is kept");
    r(&mut s);
    s.see("this is gamma");
    assert_eq!(r_hint(&s).as_deref(), Some("R:Alpha"));
    r(&mut s);
    s.see("this is alpha");
    assert_eq!(
        r_hint(&s).as_deref(),
        Some("R:Beta"),
        "the fourth press wraps"
    );
    assert_eq!(s.app.slide_position(), Some((2, 3)));
}

#[test]
fn r_walks_the_three_representations_of_a_table_and_keeps_the_sheet() {
    let (mut s, _d) = open("rc_tbl", "a.xtbl", Shape::Table, 3, 3);
    assert!(s.app.is_table_preview());
    s.see("alpha");
    assert_eq!(r_hint(&s).as_deref(), Some("R:Beta"));
    s.key('J');
    settle(&mut s);
    s.see("p2");
    r(&mut s);
    s.see("beta");
    s.see("p2");
    assert_eq!(r_hint(&s).as_deref(), Some("R:Gamma"));
    r(&mut s);
    s.see("gamma");
    s.see("p2");
    r(&mut s);
    s.see("alpha");
    s.see("p2");
    assert_eq!(r_hint(&s).as_deref(), Some("R:Beta"));
}

#[test]
fn r_walks_the_three_representations_of_a_picture_and_keeps_the_page() {
    let (mut s, _d) = open("rc_img", "a.ximg", Shape::Image, 3, 3);
    assert!(s.app.is_image_preview());
    // Red says the representation (55, 155, 255), green says the page (0, 80, 160).
    assert_eq!(red(&s), (55, 0));
    assert_eq!(r_hint(&s).as_deref(), Some("R:Beta"));
    s.key('J');
    settle(&mut s);
    assert_eq!(red(&s), (55, 80), "page 2");
    r(&mut s);
    assert_eq!(red(&s), (155, 80), "beta, still page 2");
    assert_eq!(r_hint(&s).as_deref(), Some("R:Gamma"));
    r(&mut s);
    assert_eq!(red(&s), (255, 80));
    r(&mut s);
    assert_eq!(red(&s), (55, 80), "wrapped to the first");
    assert_eq!(r_hint(&s).as_deref(), Some("R:Beta"));
}

#[test]
fn r_walks_the_three_representations_of_text_a_plugin_makes() {
    let (mut s, _d) = open("rc_txt", "a.xtxt", Shape::Text, 3, 1);
    s.see("this is alpha");
    assert_eq!(r_hint(&s).as_deref(), Some("R:Beta"));
    r(&mut s);
    s.see("this is beta");
    r(&mut s);
    s.see("this is gamma");
    r(&mut s);
    s.see("this is alpha");
}

#[test]
fn the_help_row_of_r_names_the_next_representation() {
    for (name, file, shape) in [
        ("rc_help_doc", "a.xdoc", Shape::Doc),
        ("rc_help_tbl", "a.xtbl", Shape::Table),
        ("rc_help_img", "a.ximg", Shape::Image),
        ("rc_help_txt", "a.xtxt", Shape::Text),
    ] {
        let (mut s, _d) = open(name, file, shape, 3, 1);
        s.key('?');
        s.see("Beta");
        s.key('?');
        r(&mut s);
        s.key('?');
        s.see("Gamma");
        s.key('?');
    }
}

#[test]
fn a_plugin_with_one_representation_gets_no_hint_and_r_does_nothing() {
    for (name, file, shape) in [
        ("rc_one_doc", "a.xdoc", Shape::Doc),
        ("rc_one_tbl", "a.xtbl", Shape::Table),
        ("rc_one_img", "a.ximg", Shape::Image),
        ("rc_one_txt", "a.xtxt", Shape::Text),
    ] {
        let (mut s, _d) = open(name, file, shape, 1, 1);
        assert_eq!(r_hint(&s), None, "{name}");
        let before = s.screen();
        let rgba = s.app.shown_picture_rgba();
        // Nothing is asked of the worker, so nothing is waited for.
        s.key('R');
        assert_eq!(s.screen(), before, "{name}: R changed nothing");
        assert_eq!(s.app.shown_picture_rgba(), rgba, "{name}");
        s.key('?');
        s.dont_see("Beta");
        s.key('?');
    }
}
