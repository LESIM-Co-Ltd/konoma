//! The menus `y` and `,` open must be the same in the host (`KeyMap::menu`) and in the shared
//! tables (`CoreKeys::menu`), for every preview context, with and without `[keys]` overrides.
//! The standalone plugin app reads the shared ones; konoma reads its own copy of them.

use super::*;
use konoma_host_core::keymap::{CoreContext, CoreKeys, CoreMenu};

const PAIRS: [(CoreContext, Context); 5] = [
    (CoreContext::Text, Context::Text),
    (CoreContext::Document, Context::Document),
    (CoreContext::Image, Context::Image),
    (CoreContext::Table, Context::Table),
    (CoreContext::Canvas, Context::Canvas),
];

const MENUS: [(CoreMenu, LeaderId); 3] = [
    (CoreMenu::Copy, LeaderId::Copy),
    (CoreMenu::TableCopy, LeaderId::TableCopy),
    (CoreMenu::Options, LeaderId::Options),
];

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

/// Compare every menu of every preview context; returns (menus compared, items compared).
fn assert_same_menus(label: &str, scheme: KeyScheme, c: &KeysFileConfig) -> (usize, usize) {
    let host = KeyMap::from_config(scheme, c);
    let shared = CoreKeys::from_config(scheme, c);
    let (mut menus, mut items) = (0, 0);
    for (sc, hc) in PAIRS {
        for (sm, hm) in MENUS {
            let s = shared.menu(sc, sm);
            // The host answers for a menu the context does not open too (the shared copy), so
            // compare only where the shared tables say the context has one.
            let Some(s) = s else { continue };
            let h = host
                .menu(hc, hm)
                .unwrap_or_else(|| panic!("{label}: the host has no {hm:?} menu in {hc:?}"));
            assert_eq!(
                h.title,
                Msg::from_core(s.title),
                "{label} {hc:?} {hm:?} title"
            );
            assert_eq!(
                h.items.len(),
                s.items.len(),
                "{label} {hc:?} {hm:?}: item count"
            );
            for (hi, si) in h.items.iter().zip(&s.items) {
                assert_eq!(hi.key, si.key, "{label} {hc:?} {hm:?}");
                assert_eq!(hi.action, Action::from(si.action), "{label} {hc:?} {hm:?}");
                assert_eq!(hi.label, Msg::from_core(si.label), "{label} {hc:?} {hm:?}");
            }
            menus += 1;
            items += s.items.len();
        }
    }
    (menus, items)
}

fn scenarios() -> Vec<(&'static str, KeysFileConfig)> {
    vec![
        ("defaults", cfg(&[])),
        (
            "add",
            cfg(&[("text", &[("y x", "copy_full"), (", s", "sort_size")])]),
        ),
        (
            "replace",
            cfg(&[("text", &[("y n", "copy_parent"), (", p", "toggle_wrap")])]),
        ),
        (
            "remove",
            cfg(&[("document", &[("y c", "noop"), (", w", "noop")])]),
        ),
        (
            "table",
            cfg(&[(
                "table",
                &[("y x", "copy_name"), ("y c", "noop"), (", z", "copy_full")],
            )]),
        ),
        (
            "global",
            cfg(&[(
                "global",
                &[
                    ("y x", "copy_full"),
                    (", z", "toggle_wrap"),
                    ("y n", "noop"),
                ],
            )]),
        ),
        (
            "global-then-context",
            cfg(&[
                ("global", &[("y x", "copy_full")]),
                ("image", &[("y x", "copy_name"), ("y q", "copy_relative")]),
            ]),
        ),
        (
            "old-name",
            cfg(&[
                ("preview_text", &[("y x", "copy_full")]),
                ("preview_image", &[(", z", "cycle_path_style")]),
                ("preview_table", &[("y z", "table_copy_row")]),
            ]),
        ),
        (
            "new-name-beats-old",
            cfg(&[
                ("preview_text", &[("y x", "copy_full")]),
                ("text", &[("y x", "copy_name")]),
            ]),
        ),
        (
            "key-taken-then-chord",
            cfg(&[("text", &[("y", "copy_full"), ("y x", "copy_name")])]),
        ),
        (
            "canvas",
            cfg(&[(
                "canvas",
                &[
                    ("y x", "copy_full"),
                    (", s", "sort_name"),
                    ("space x", "quit"),
                ],
            )]),
        ),
        (
            "refused",
            cfg(&[("text", &[("z x", "copy_full"), ("space x", "copy_full")])]),
        ),
        (
            "code-block-and-sorts",
            cfg(&[(
                "text",
                &[
                    ("y k", "copy_code_block"),
                    (", n", "sort_name"),
                    (", m", "sort_modified"),
                    (", e", "sort_ext"),
                ],
            )]),
        ),
        (
            "wide-and-cjk-keys",
            cfg(&[(
                "text",
                &[
                    ("y あ", "copy_full"),
                    ("あ", "copy_full"),
                    ("ｙ x", "copy_full"),
                    (", 漢", "toggle_wrap"),
                ],
            )]),
        ),
        (
            "wide-and-cjk-keys-global",
            cfg(&[("global", &[("y あ", "copy_full"), ("ｙ x", "copy_name")])]),
        ),
    ]
}

#[test]
fn the_host_and_the_shared_tables_have_the_same_menus_in_every_scenario() {
    for scheme in [KeyScheme::Vim, KeyScheme::Less] {
        let (mut menus, mut items) = (0, 0);
        let mut n = 0;
        for (label, c) in scenarios() {
            let (m, i) = assert_same_menus(label, scheme, &c);
            menus += m;
            items += i;
            n += 1;
        }
        // 5 contexts x 3 menus per scenario.
        assert_eq!(menus, n * 15);
        assert!(n >= 15, "{n} scenarios");
        assert!(menus >= 15 * 10, "only {menus} menus were compared");
        assert!(items >= 15 * 30, "only {items} items were compared");
    }
}

#[test]
fn the_overrides_really_changed_something_in_both() {
    // A comparison of two unchanged menus would pass: pin that each scenario moved the menus.
    let defaults = cfg(&[]);
    let base = KeyMap::from_config(KeyScheme::Vim, &defaults);
    for (label, c) in scenarios() {
        if matches!(label, "defaults" | "refused" | "key-taken-then-chord") {
            continue;
        }
        let host = KeyMap::from_config(KeyScheme::Vim, &c);
        let moved = PAIRS.iter().any(|&(_, hc)| {
            MENUS.iter().any(|&(_, hm)| {
                let a: Vec<_> = host
                    .menu(hc, hm)
                    .map(|m| m.items.iter().map(|i| (i.key, i.action)).collect())
                    .unwrap_or_default();
                let b: Vec<_> = base
                    .menu(hc, hm)
                    .map(|m| m.items.iter().map(|i| (i.key, i.action)).collect())
                    .unwrap_or_default();
                a != b
            })
        });
        assert!(moved, "{label}: the host's menus did not change");
        let shared = CoreKeys::from_config(KeyScheme::Vim, &c);
        assert_ne!(
            shared.menus,
            CoreKeys::defaults(KeyScheme::Vim).menus,
            "{label}: the shared menus did not change"
        );
    }
}

#[test]
fn a_menu_the_tree_shares_follows_a_global_entry_in_the_host() {
    // The tree's `y` menu is the shared copy menu: a global entry reaches it too.
    let host = KeyMap::from_config(KeyScheme::Vim, &cfg(&[("global", &[("y x", "copy_full")])]));
    let tree = host.menu(Context::Tree, LeaderId::Copy).unwrap();
    assert_eq!(
        tree.find(KeyPress::ch('x')),
        Some(Action::CopyPath(CopyKind::Full))
    );
    let text = host.menu(Context::Text, LeaderId::Copy).unwrap();
    assert_eq!(text.items.len(), tree.items.len());
}

#[test]
fn a_host_only_action_still_goes_into_a_preview_menu_of_the_host() {
    // The shared tables ignore the name; konoma keeps accepting it, with its own label.
    let host = KeyMap::from_config(
        KeyScheme::Vim,
        &cfg(&[("text", &[("y x", "toggle_hidden")])]),
    );
    let m = host.menu(Context::Text, LeaderId::Copy).unwrap();
    let it = m.items.iter().find(|i| i.key == KeyPress::ch('x')).unwrap();
    assert_eq!(it.action, Action::ToggleHidden);
    assert_eq!(it.label, Msg::HintHidden);
    assert!(host.warnings.is_empty(), "{:?}", host.warnings);
}
