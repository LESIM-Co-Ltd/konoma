//! The host keymap and the shared key layers (`konoma_host_core::keymap`) must agree.
//!
//! The standalone plugin app resolves keys with `CoreKeys`; konoma resolves them with `KeyMap`.
//! For every context the two have in common and every key a person can press, whatever the
//! shared tables answer the host must answer too (the same action, the same menu, or unbound),
//! with and without `[keys]` overrides. The host may add host-only commands on keys the shared
//! tables leave unbound (searching, marks, the project screens), never a shared command.

use super::*;
use konoma_host_core::keymap::{
    CoreAction, CoreBinding, CoreContext, CoreKeys, CoreMenu, CoreResolution,
};

const PAIRS: [(CoreContext, Context); 5] = [
    (CoreContext::Text, Context::Text),
    (CoreContext::Document, Context::Document),
    (CoreContext::Image, Context::Image),
    (CoreContext::Table, Context::Table),
    (CoreContext::Canvas, Context::Canvas),
];

fn probes() -> Vec<KeyPress> {
    let mut v: Vec<KeyPress> = (0x20u8..0x7f)
        .map(|b| KeyPress::ch(char::from(b)))
        .collect();
    v.extend(('a'..='z').map(KeyPress::ctrl_ch));
    for code in [
        KeyCode::Enter,
        KeyCode::Tab,
        KeyCode::Esc,
        KeyCode::Backspace,
        KeyCode::Up,
        KeyCode::Down,
        KeyCode::Left,
        KeyCode::Right,
        KeyCode::Home,
        KeyCode::End,
        KeyCode::PageUp,
        KeyCode::PageDown,
    ] {
        v.push(KeyPress::key(code));
    }
    v
}

fn cfg(tables: &[(&str, &[(&str, &str)])]) -> KeysFileConfig {
    KeysFileConfig {
        surfaces: tables
            .iter()
            .map(|(n, es)| {
                (
                    n.to_string(),
                    es.iter()
                        .map(|(k, a)| (k.to_string(), a.to_string()))
                        .collect(),
                )
            })
            .collect(),
    }
}

fn expected(r: CoreResolution) -> Resolution {
    match r {
        CoreResolution::Action(a) => Resolution::Action(a.into()),
        CoreResolution::Menu(CoreMenu::Actions) => Resolution::EnterLeader(LeaderId::Actions),
        CoreResolution::Menu(CoreMenu::Copy) => Resolution::EnterLeader(LeaderId::Copy),
        CoreResolution::Menu(CoreMenu::TableCopy) => Resolution::EnterLeader(LeaderId::TableCopy),
        CoreResolution::Menu(CoreMenu::Options) => Resolution::EnterLeader(LeaderId::Options),
        CoreResolution::Unbound => Resolution::Unbound,
    }
}

/// Compare the two for every pair and probe; returns how many keys the shared tables answered.
fn assert_agree(label: &str, scheme: KeyScheme, c: &KeysFileConfig) -> usize {
    let host = KeyMap::from_config(scheme, c);
    let shared = CoreKeys::from_config(scheme, c);
    let mut answered = 0;
    for (sc, hc) in PAIRS {
        for kp in probes() {
            let s = shared.resolve(sc, kp);
            let h = host.resolve(hc, None, kp);
            if s == CoreResolution::Unbound {
                // The host may use the key for a host-only command, never for a shared one and
                // never for a menu.
                match h {
                    Resolution::Action(a) => assert!(
                        a.core().is_none(),
                        "{label}/{scheme:?}: {hc:?} {kp:?}: the host runs the shared command {a:?} where the shared tables leave the key unbound"
                    ),
                    Resolution::EnterLeader(l) => panic!(
                        "{label}/{scheme:?}: {hc:?} {kp:?}: the host opens the menu {l:?} where the shared tables leave the key unbound"
                    ),
                    _ => {}
                }
            } else {
                answered += 1;
                assert_eq!(
                    h,
                    expected(s),
                    "{label}/{scheme:?}: {hc:?} {kp:?}: host and shared tables disagree"
                );
            }
        }
    }
    answered
}

fn scenarios() -> Vec<(&'static str, KeysFileConfig)> {
    vec![
        ("defaults", cfg(&[])),
        (
            "non-ascii-key-names",
            cfg(&[(
                "text",
                &[
                    ("\u{3042}", "page_next"),
                    ("\u{00e9}", "navigate:down"),
                    ("\u{4e2d}", "noop"),
                    ("ctrl-\u{00e9}", "page_next"),
                    ("\u{00e9}\u{0301}", "page_next"),
                ],
            )]),
        ),
        (
            "non-ascii-keys-in-global-and-types",
            cfg(&[
                ("global", &[("\u{00fc}", "quit"), ("\u{6f22}", "noop")]),
                ("image", &[("\u{00f1}", "image_zoom_in")]),
                ("table", &[("\u{1f600}", "toggle_outline")]),
                (
                    "text",
                    &[("\u{3042}\u{3044}", "page_next"), ("ctrl-\u{3042}", "noop")],
                ),
            ]),
        ),
        ("rebind", cfg(&[("text", &[("x", "page_next")])])),
        (
            "rebind-several-contexts",
            cfg(&[
                (
                    "text",
                    &[("x", "image_zoom_in"), ("z", "navigate:half_down")],
                ),
                ("image", &[("x", "page_prev")]),
                ("table", &[("x", "toggle_outline")]),
                ("canvas", &[("x", "request_edit")]),
            ]),
        ),
        ("unbind-inherited", cfg(&[("text", &[("j", "noop")])])),
        (
            "unbind-type-layer",
            cfg(&[("image", &[("J", "noop"), ("+", "disabled")])]),
        ),
        (
            "unbind-own",
            cfg(&[("table", &[("O", "noop"), ("q", "noop")])]),
        ),
        (
            "unbind-verbs",
            cfg(&[("text", &[("e", "noop"), ("o", "noop")])]),
        ),
        (
            "unbind-global-is-a-noop-in-a-context",
            cfg(&[("text", &[("Q", "noop")])]),
        ),
        (
            "unbind-in-global",
            cfg(&[("global", &[("Q", "noop"), ("?", "noop")])]),
        ),
        (
            "global-rebind",
            cfg(&[("global", &[("X", "page_next"), ("t", "toggle_help")])]),
        ),
        (
            "key-moved-between-layers",
            cfg(&[(
                "text",
                &[
                    ("j", "navigate:up"),
                    ("k", "navigate:down"),
                    ("g", "navigate:bottom"),
                ],
            )]),
        ),
        (
            "type-key-moved-to-layer-one-key",
            cfg(&[("image", &[("h", "navigate:down"), ("j", "image_zoom_in")])]),
        ),
        (
            "take-a-global-key",
            cfg(&[("table", &[("t", "page_next"), ("Q", "page_next")])]),
        ),
        (
            "take-a-menu-key",
            cfg(&[(
                "text",
                &[
                    ("space", "page_next"),
                    ("y", "page_next"),
                    (",", "page_next"),
                ],
            )]),
        ),
        (
            "take-a-menu-key-table",
            cfg(&[("table", &[("y", "page_next")])]),
        ),
        (
            "fixed-keys-ignored",
            cfg(&[(
                "text",
                &[
                    ("enter", "page_next"),
                    ("esc", "quit"),
                    ("home", "page_next"),
                ],
            )]),
        ),
        ("chords-ignored", cfg(&[("text", &[("y x", "page_next")])])),
        ("tab-goto-ignored", cfg(&[("text", &[("w", "tab_goto")])])),
        (
            "ctrl-keys",
            cfg(&[("text", &[("ctrl-x", "page_next"), ("ctrl-f", "noop")])]),
        ),
        (
            "old-surface-names",
            cfg(&[
                ("preview_text", &[("x", "page_next"), ("n", "page_next")]),
                ("preview_image", &[("x", "page_prev")]),
                ("preview_table", &[("x", "toggle_outline")]),
            ]),
        ),
        (
            "old-name-beaten-by-new",
            cfg(&[
                ("preview_text", &[("x", "page_next")]),
                ("text", &[("x", "page_prev")]),
            ]),
        ),
        (
            "aliases",
            cfg(&[("text", &[("x", "pdf_next_page"), ("z", "sheet_prev")])]),
        ),
        (
            "host-only-actions-and-typos",
            cfg(&[(
                "text",
                &[
                    ("x", "no_such_action"),
                    ("z", "search_start"),
                    ("w", "mark_set"),
                ],
            )]),
        ),
        (
            "host-only-key-rebound-to-a-shared-action",
            cfg(&[("text", &[("n", "page_next"), ("r", "navigate:down")])]),
        ),
        (
            "canvas-takes-motion",
            cfg(&[("canvas", &[("j", "navigate:down"), ("k", "navigate:up")])]),
        ),
    ]
}

#[test]
fn the_host_and_the_shared_tables_agree_in_every_scenario_and_scheme() {
    for (label, c) in scenarios() {
        for scheme in [KeyScheme::Vim, KeyScheme::Less] {
            assert_agree(label, scheme, &c);
        }
    }
}

/// Non-ASCII key names are a person's typing: every one must load without a panic, and the host
/// and the shared side must take or refuse each the same way, which is seen by pressing it.
#[test]
fn non_ascii_key_names_are_accepted_or_refused_alike() {
    let probes_na = [
        KeyPress::ch('\u{3042}'),
        KeyPress::ch('\u{00e9}'),
        KeyPress::ch('\u{4e2d}'),
        KeyPress::ch('\u{00fc}'),
        KeyPress::ctrl_ch('\u{00e9}'),
    ];
    for scheme in [KeyScheme::Vim, KeyScheme::Less] {
        let c = cfg(&[
            (
                "text",
                &[
                    ("\u{3042}", "page_next"),
                    ("\u{00e9}", "navigate:down"),
                    ("\u{4e2d}", "noop"),
                    ("ctrl-\u{00e9}", "page_next"),
                ],
            ),
            ("global", &[("\u{00fc}", "quit")]),
        ]);
        let host = KeyMap::from_config(scheme, &c);
        let shared = CoreKeys::from_config(scheme, &c);
        for kp in probes_na {
            let s = shared.resolve(CoreContext::Text, kp);
            let h = host.resolve(Context::Text, None, kp);
            if s == CoreResolution::Unbound {
                assert!(
                    !matches!(h, Resolution::EnterLeader(_))
                        && !matches!(h, Resolution::Action(a) if a.core().is_some()),
                    "{scheme:?} {kp:?}: {h:?} where the shared side is unbound"
                );
            } else {
                assert_eq!(h, expected(s), "{scheme:?} {kp:?}");
            }
        }
    }
}

#[test]
fn a_menu_the_shared_side_binds_is_the_same_menu_in_the_host() {
    // Menu vs menu, and Unbound vs menu: the menu keys under a rebind that frees or takes them.
    let host = KeyMap::defaults(KeyScheme::Vim);
    let shared = CoreKeys::defaults(KeyScheme::Vim);
    let mut menus = 0;
    for (sc, hc) in PAIRS {
        for kp in probes() {
            if let CoreResolution::Menu(m) = shared.resolve(sc, kp) {
                menus += 1;
                assert_eq!(
                    host.resolve(hc, None, kp),
                    expected(CoreResolution::Menu(m)),
                    "{hc:?} {kp:?}"
                );
            }
        }
    }
    assert!(menus >= 5, "only {menus} menu keys were compared");
    // Freed in the shared tables: the host must not still open it.
    let c = cfg(&[("text", &[("space", "noop"), ("y", "noop")])]);
    assert_agree("menu-freed", KeyScheme::Vim, &c);
}

#[test]
fn the_comparison_covers_a_real_number_of_keys() {
    // A comparison that skips everything would pass: pin that the shared tables answer a lot.
    let n = assert_agree("defaults", KeyScheme::Vim, &cfg(&[]));
    assert!(n >= 100, "only {n} (context, key) pairs were compared");
}

#[test]
fn every_key_of_the_shared_tables_is_compared() {
    let keys = CoreKeys::defaults(KeyScheme::Vim);
    let mut seen = 0;
    let host = KeyMap::defaults(KeyScheme::Vim);
    for (sc, hc) in PAIRS {
        let mut ks: Vec<KeyPress> = keys.global.keys().copied().collect();
        ks.extend(keys.basic.keys().copied());
        for vt in sc.view_types() {
            ks.extend(keys.types[vt].keys().copied());
        }
        ks.extend(keys.contexts[&sc].keys().copied());
        for kp in ks {
            let s = keys.resolve(sc, kp);
            if s == CoreResolution::Unbound {
                continue; // the canvas does not inherit layer 1
            }
            seen += 1;
            assert_eq!(host.resolve(hc, None, kp), expected(s), "{hc:?} {kp:?}");
        }
    }
    assert!(seen >= 100, "only {seen} shared keys were compared");
}

#[test]
fn a_shared_table_key_that_the_host_overrides_is_not_possible() {
    // The host builds its tables from the shared ones; a host-only insert on top of a shared
    // key would break agreement silently, so walk the host tables too: every host binding on a
    // key that the shared tables bind in the same context is the shared one.
    let host = KeyMap::defaults(KeyScheme::Vim);
    let shared = CoreKeys::defaults(KeyScheme::Vim);
    for (sc, hc) in PAIRS {
        for (kp, b) in host.contexts.get(&hc).into_iter().flatten() {
            match shared.resolve(sc, *kp) {
                CoreResolution::Unbound => {}
                s => assert_eq!(
                    host.resolve(hc, None, *kp),
                    expected(s),
                    "{hc:?} {kp:?} ({b:?})"
                ),
            }
        }
    }
}

#[test]
fn core_action_names_agree_with_the_host_tables() {
    for a in CoreAction::all() {
        let host: Action = a.into();
        assert_eq!(host.core(), Some(a), "{a:?}");
        assert_eq!(action_name(host), a.name(), "{a:?}");
        if !matches!(a, CoreAction::TabGoto(_)) {
            assert_eq!(action_from_str(&a.name()), Some(host), "{}", a.name());
        } else {
            assert_eq!(action_from_str("tab_goto"), None);
        }
    }
    for (old, new) in [
        ("pdf_next_page", Action::PageNext),
        ("sheet_next", Action::PageNext),
        ("media_diff_page_next", Action::PageNext),
        ("pdf_prev_page", Action::PagePrev),
        ("sheet_prev", Action::PagePrev),
        ("media_diff_page_prev", Action::PagePrev),
    ] {
        assert_eq!(action_from_str(old), Some(new), "{old}");
    }
}

#[test]
fn a_host_only_action_is_not_a_core_action_and_keeps_its_name() {
    for n in [
        "search_start",
        "mark_set",
        "refresh",
        "tree_descend",
        "noop",
        "cycle_representation",
    ] {
        let a = action_from_str(n).unwrap_or_else(|| panic!("{n} should be a host action"));
        assert_eq!(a.core(), None, "{n}");
        assert_eq!(CoreAction::from_name(n), None, "{n}");
    }
}

#[test]
fn the_shared_binding_converts_to_the_matching_host_binding() {
    assert_eq!(
        Binding::from(CoreBinding::Menu(CoreMenu::Options)),
        Binding::Leader(LeaderId::Options)
    );
    assert_eq!(Binding::from(CoreBinding::Unbound), Binding::Unbound);
    assert_eq!(
        Binding::from(CoreBinding::Run(CoreAction::Quit)),
        Binding::Run(Action::Quit)
    );
}
