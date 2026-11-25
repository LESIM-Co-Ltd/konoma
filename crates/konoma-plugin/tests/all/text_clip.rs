//! The texts a plugin hands the host are cut to `MAX_TEXT_CHARS` when the host takes them in.

use std::sync::Arc;

use konoma_plugin::manifest::Unavailable;
use konoma_plugin::prelude::*;
use konoma_plugin::registry::{PluginOrigin, Registry};
use konoma_plugin::text::{clip_text, MAX_TEXT_CHARS};
use konoma_plugin::view::{Layout, PageLayout, ReprDecl};

fn long(c: char) -> String {
    std::iter::repeat_n(c, 100_000).collect()
}

#[test]
fn a_text_within_the_limit_is_untouched() {
    let s: String = std::iter::repeat_n('a', MAX_TEXT_CHARS).collect();
    assert_eq!(clip_text(&s), s.as_str());
    assert!(matches!(clip_text(&s), std::borrow::Cow::Borrowed(_)));
    assert_eq!(clip_text(""), "");
}

#[test]
fn a_longer_text_is_cut_to_the_limit_with_an_ellipsis() {
    let s = long('x');
    let c = clip_text(&s);
    assert_eq!(c.chars().count(), MAX_TEXT_CHARS);
    assert!(c.ends_with('…'));
    assert!(c.starts_with("xxx"));
    let one_over: String = std::iter::repeat_n('a', MAX_TEXT_CHARS + 1).collect();
    assert_eq!(clip_text(&one_over).chars().count(), MAX_TEXT_CHARS);
}

#[test]
fn a_cut_never_lands_inside_a_character() {
    // Three-byte and four-byte characters around the cut.
    for ch in ['あ', '😀', 'é'] {
        let s = long(ch);
        let c = clip_text(&s);
        assert_eq!(c.chars().count(), MAX_TEXT_CHARS);
        assert!(c.ends_with('…'));
    }
}

#[test]
fn every_language_of_a_tr_is_cut() {
    let tr = Tr::new(long('e')).ja(long('j')).with("fr", long('f'));
    let c = tr.clipped();
    assert_eq!(c.en().chars().count(), MAX_TEXT_CHARS);
    assert_eq!(c.get("ja").chars().count(), MAX_TEXT_CHARS);
    assert_eq!(c.get("fr").chars().count(), MAX_TEXT_CHARS);
    assert_eq!(
        Tr::new("short").ja("短い").clipped(),
        Tr::new("short").ja("短い")
    );
}

#[test]
fn a_failure_is_cut_in_message_and_detail() {
    let f = Failure::external(Tr::new(long('m')).ja(long('n'))).with_detail(long('d'));
    let c = f.clipped();
    assert_eq!(c.message.en().chars().count(), MAX_TEXT_CHARS);
    assert_eq!(c.message.get("ja").chars().count(), MAX_TEXT_CHARS);
    assert_eq!(c.detail.as_deref().unwrap().chars().count(), MAX_TEXT_CHARS);
    assert_eq!(c.kind, f.kind);
}

#[test]
fn a_layout_is_cut_in_repr_labels_and_page_names() {
    let layout = Layout {
        reprs: vec![ReprDecl::new("one", Tr::new(long('r')))],
        default_repr: Some("one".into()),
        default_repr_without_pixels: None,
        pages: Some(PageLayout::Separate {
            count: Some(2),
            titles: vec![long('p'), "Sheet2".into()],
        }),
    };
    let c = layout.clipped();
    assert_eq!(c.reprs[0].label.en().chars().count(), MAX_TEXT_CHARS);
    assert_eq!(c.reprs[0].id, "one");
    let Some(PageLayout::Separate { count, titles }) = c.pages else {
        panic!("pages kept");
    };
    assert_eq!(count, Some(2));
    assert_eq!(titles[0].chars().count(), MAX_TEXT_CHARS);
    assert_eq!(titles[1], "Sheet2");
}

struct Loud;

impl Plugin for Loud {
    fn manifest(&self) -> Manifest {
        Manifest::new("loud", "0.1.0")
            .name(Tr::new(long('n')))
            .action(
                Act::new("go")
                    .key("x")
                    .label(Tr::new(long('l')).ja(long('m')))
                    .help(Tr::new(long('h'))),
            )
            .setting(SettingDecl::new("k", Value::Bool(true), Tr::new(long('s'))))
            .unavailable(Unavailable::Notice(Tr::new(long('u'))))
    }
    fn open(&self, _: Input, _: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        Err(Failure::unsupported(Tr::new("no")))
    }
}

#[test]
fn the_manifest_the_registry_keeps_has_cut_texts() {
    let mut r = Registry::new();
    r.register(Arc::new(Loud), PluginOrigin::User).unwrap();
    let m = r.manifest("loud").unwrap();
    let max = MAX_TEXT_CHARS;
    assert_eq!(m.name.en().chars().count(), max);
    let act = &m.actions[0];
    assert_eq!(act.label.en().chars().count(), max);
    assert_eq!(act.label.get("ja").chars().count(), max);
    assert_eq!(act.help.en().chars().count(), max);
    assert_eq!(act.key.as_deref(), Some("x"));
    assert_eq!(m.settings[0].help.en().chars().count(), max);
    let Unavailable::Notice(t) = &m.unavailable else {
        panic!("notice kept");
    };
    assert_eq!(t.en().chars().count(), max);
}

// ---- what is stripped and what is counted -------------------------------------------------------

#[test]
fn control_characters_and_the_kitty_placeholder_are_stripped_from_a_text() {
    // A newline and a tab stay (a failure's message may span lines); the rest of the C0 / C1
    // controls, DEL and U+10EEEE go.
    let s = "a\u{1b}[2Jb\u{7}c\nd\te\u{7f}f\u{85}g\u{10EEEE}h";
    assert_eq!(clip_text(s), "a[2Jbc\nd\tefgh");
    // Clean text is still borrowed (no copy per frame).
    assert!(matches!(clip_text("plain"), std::borrow::Cow::Borrowed(_)));
    // It applies to every door: a Tr, a failure, a page title.
    let tr = Tr::new("x\u{10EEEE}y").ja("\u{1b}日本").clipped();
    assert_eq!(tr.en(), "xy");
    assert_eq!(tr.get("ja"), "日本");
    let f = Failure::external(Tr::new("m\u{10EEEE}")).with_detail("d\u{1b}x");
    let c = f.clipped();
    assert_eq!(c.message.en(), "m");
    assert_eq!(c.detail.as_deref(), Some("dx"));
    let layout = Layout {
        reprs: vec![],
        default_repr: None,
        default_repr_without_pixels: None,
        pages: Some(PageLayout::Separate {
            count: Some(1),
            titles: vec!["p\u{10EEEE}\u{1b}1".into()],
        }),
    };
    match layout.clipped().pages {
        Some(PageLayout::Separate { titles, .. }) => assert_eq!(titles[0], "p1"),
        _ => panic!(),
    }
}

#[test]
fn a_tr_keeps_a_bounded_number_of_other_languages() {
    let mut tr = Tr::new("en");
    for i in 0..10_000 {
        tr = tr.with(format!("l{i}"), "t");
    }
    let c = tr.clipped();
    // The first ones stay readable, the rest are gone, `get` stays a short walk.
    assert_eq!(c.get("l0"), "t");
    assert_eq!(c.get("l15"), "t");
    assert_eq!(c.get("l16"), "en");
    assert_eq!(c.get("l9999"), "en");
    // A language name is bounded as well.
    let long_name = Tr::new("e").with("x".repeat(1_000_000), "t").clipped();
    assert_eq!(long_name.en(), "e");
}

#[test]
fn a_layout_is_bounded_in_representations_pages_and_ids() {
    use konoma_plugin::view::{MAX_LAYOUT_PAGES, MAX_LAYOUT_REPRS};
    let reprs: Vec<ReprDecl> = (0..10_000)
        .map(|i| ReprDecl::new(format!("r{i}"), Tr::new("l")))
        .chain([ReprDecl::new(long('i'), Tr::new("huge id"))])
        .collect();
    let layout = Layout {
        reprs,
        default_repr: Some(long('d')),
        default_repr_without_pixels: Some("r1".into()),
        pages: Some(PageLayout::Separate {
            count: Some(4_000_000_000),
            titles: vec!["t".into(); MAX_LAYOUT_PAGES + 5_000],
        }),
    };
    let c = layout.clipped();
    assert_eq!(c.reprs.len(), MAX_LAYOUT_REPRS);
    assert!(c
        .reprs
        .iter()
        .all(|r| r.id.chars().count() <= MAX_TEXT_CHARS));
    assert_eq!(c.default_repr, None, "an over-long id names nothing");
    assert_eq!(c.default_repr_without_pixels.as_deref(), Some("r1"));
    match c.pages {
        Some(PageLayout::Separate { count, titles }) => {
            assert_eq!(count, Some(MAX_LAYOUT_PAGES));
            assert_eq!(titles.len(), MAX_LAYOUT_PAGES);
        }
        _ => panic!(),
    }
    let anchored = Layout {
        pages: Some(PageLayout::Anchored {
            heading_level: 2,
            count: usize::MAX,
        }),
        ..Layout::default()
    };
    match anchored.clipped().pages {
        Some(PageLayout::Anchored { count, .. }) => assert_eq!(count, MAX_LAYOUT_PAGES),
        _ => panic!(),
    }
    // Unknown stays unknown.
    let unknown = Layout {
        pages: Some(PageLayout::Separate {
            count: None,
            titles: vec![],
        }),
        ..Layout::default()
    };
    assert!(matches!(
        unknown.clipped().pages,
        Some(PageLayout::Separate { count: None, .. })
    ));
}
