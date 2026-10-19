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
