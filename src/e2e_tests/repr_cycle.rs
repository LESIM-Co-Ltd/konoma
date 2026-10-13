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

// ---- the names of the representations are the plugin's own -------------------------------------

use crate::plugins::test_views::{DocFileOrdered, Named};

fn open_named(
    name: &str,
    file: &str,
    plugin: Arc<dyn Plugin>,
    media: bool,
) -> (Sim, crate::test_support::TmpDir) {
    let dir = sandbox(name);
    std::fs::write(dir.join(file), "# Title\n\nbody text\n").unwrap();
    let cfg = cfg_for(name, plugin);
    let mut s = Sim::with_config_sized(&canon(&dir), cfg, 100, 30);
    if media {
        s = s.with_media();
    }
    s.select(file);
    s.enter();
    settle(&mut s);
    (s, dir)
}

/// A table, a text and a picture plugin that call their first representation `source` are walked
/// by the host like any other: `R` goes to the next one, and a reload of the file (a change on
/// disk) keeps the representation.
#[test]
fn a_representation_called_source_is_just_a_name_for_a_table_a_text_and_a_picture() {
    for (name, file, shape) in [
        ("rcn_tbl", "a.ntbl", Shape::Table),
        ("rcn_txt", "a.ntxt", Shape::Text),
        ("rcn_img", "a.nimg", Shape::Image),
    ] {
        let ext: &'static str = Box::leak(
            file.rsplit('.')
                .next()
                .unwrap()
                .to_string()
                .into_boxed_str(),
        );
        let id: &'static str = Box::leak(format!("x-{name}").into_boxed_str());
        let (mut s, _d) = open_named(
            name,
            file,
            Arc::new(Named {
                id,
                ext,
                shape,
                ids: &["source", "pretty"],
            }),
            matches!(shape, Shape::Image),
        );
        assert_eq!(r_hint(&s).as_deref(), Some("R:Show pretty"), "{name}");
        let first = |s: &Sim| match shape {
            Shape::Image => Some(red(s)),
            _ => None,
        };
        let before = (s.screen(), first(&s));
        r(&mut s);
        assert_eq!(r_hint(&s).as_deref(), Some("R:Show source"), "{name}");
        assert_ne!(
            (s.screen(), first(&s)),
            before,
            "{name}: R changed the screen"
        );
        let pretty = (s.screen(), first(&s));
        match shape {
            Shape::Table => s.see("pretty"),
            Shape::Text => s.see("this is pretty"),
            _ => assert_eq!(first(&s), Some((155, 0)), "{name}"),
        }
        // The file changes on disk and is read again: the representation is kept.
        let path = _d.join(file);
        std::fs::write(&path, "changed\n").unwrap();
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(30))
            .unwrap();
        s.app.reload_preview();
        settle(&mut s);
        assert_eq!(
            (s.screen(), first(&s)),
            pretty,
            "{name}: kept across a reload"
        );
        assert_eq!(r_hint(&s).as_deref(), Some("R:Show source"), "{name}");
        r(&mut s);
        assert_eq!(
            r_hint(&s).as_deref(),
            Some("R:Show pretty"),
            "{name}: wraps"
        );
    }
}

/// A document that declares `rendered` and `source` in the other order is the same standard shape:
/// the host's own wording and its own toggle (the source in the windowed reader).
#[test]
fn a_document_declaring_source_before_rendered_gets_the_standard_toggle() {
    let (mut s, _d) = open_named(
        "rcn_doc_rev",
        "a.ndoc",
        Arc::new(DocFileOrdered {
            id: "x-rcn-doc-rev",
            ext: "ndoc",
            ids: &["source", "rendered"],
        }),
        false,
    );
    // The host's wording ("Raw source" / "Rendered" of the catalog), not "Show ..." of the plugin.
    assert!(s.app.host_source_view());
    assert!(
        s.app.repr_step().is_none(),
        "the host's own toggle, not the walk"
    );
    let hint = crate::ui::preview::footer_hints(&s.app)
        .into_iter()
        .find(|h| h.starts_with("R:"))
        .expect("R is offered");
    assert!(!hint.contains("Show"), "{hint}");
    s.key('R');
    settle(&mut s);
    assert!(s.app.is_md_raw(), "R switched to the source view");
    s.key('R');
    settle(&mut s);
    assert!(!s.app.is_md_raw());
}

/// A document that declares one representation gets no `R` hint, and the key does nothing; a
/// document that names two others is walked by the host with the plugin's labels.
#[test]
fn a_document_with_one_representation_has_no_r_and_one_with_other_names_is_walked() {
    let (mut s, _d) = open_named(
        "rcn_doc_one",
        "a.ndoc",
        Arc::new(DocFileOrdered {
            id: "x-rcn-doc-one",
            ext: "ndoc",
            ids: &["rendered"],
        }),
        false,
    );
    assert!(!s.app.host_source_view());
    assert_eq!(r_hint(&s), None);
    let before = s.screen();
    s.key('R');
    settle(&mut s);
    assert_eq!(s.screen(), before, "R changed nothing");
    assert!(!s.app.is_md_raw());

    let (s, _d) = open_named(
        "rcn_doc_two",
        "a.ndoc",
        Arc::new(DocFileOrdered {
            id: "x-rcn-doc-two",
            ext: "ndoc",
            ids: &["source", "pretty"],
        }),
        false,
    );
    assert!(!s.app.host_source_view(), "not the standard pair of names");
    assert_eq!(r_hint(&s).as_deref(), Some("R:Show pretty"));
    assert!(
        !s.app.is_md_raw(),
        "`source` here is a name of the plugin's"
    );
}
