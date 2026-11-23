//! The operations only konoma runs (`HostOp`): the shared tables hold their default keys and read
//! their names from `[keys]`, and konoma's keymap is built from them. The two must say the same
//! thing for every key, with and without `[keys]` overrides: where the shared side binds a host
//! operation the host runs the matching action, and where it does not the host runs none.

use super::*;
use konoma_host_core::keymap::{CoreContext, CoreKeys, HostOp};

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

/// The host action of every operation that exists in this build.
fn host_ops() -> Vec<(HostOp, Action)> {
    HostOp::ALL
        .into_iter()
        .filter_map(|o| host_op_action(o).map(|a| (o, a)))
        .collect()
}

/// Compare the two for every pair and probe; returns how many keys carried a host operation.
fn assert_agree(label: &str, scheme: KeyScheme, c: &KeysFileConfig) -> usize {
    let host = KeyMap::from_config(scheme, c);
    let shared = CoreKeys::from_config(scheme, c);
    let ops = host_ops();
    let mut compared = 0;
    for (sc, hc) in PAIRS {
        for kp in probes() {
            let h = host.resolve(hc, None, kp);
            let host_op = match h {
                Resolution::Action(a) => ops.iter().find(|(_, x)| *x == a).map(|(o, _)| *o),
                _ => None,
            };
            let mut want = shared.host_op(sc, kp);
            // `!` has no action in a build without `git`: the host leaves it unbound.
            if want.is_some_and(|o| host_op_action(o).is_none()) {
                want = None;
            }
            if want.is_some() {
                compared += 1;
            }
            assert_eq!(
                host_op, want,
                "{label}/{scheme:?}: {hc:?} {kp:?}: host {h:?}, shared {want:?}"
            );
        }
    }
    compared
}

fn scenarios() -> Vec<(&'static str, KeysFileConfig)> {
    vec![
        ("defaults", cfg(&[])),
        ("rebind", cfg(&[("text", &[("x", "search_start")])])),
        (
            "every-name-in-one-context",
            cfg(&[(
                "document",
                &[
                    ("x", "search_next"),
                    ("z", "toggle_markdown_raw"),
                    ("w", "mark_jump"),
                    ("u", "image_return_to_diff"),
                    ("i", "open_link_new_tab"),
                    ("ctrl-x", "preview_next_file"),
                ],
            )]),
        ),
        (
            "unbind-a-host-key",
            cfg(&[(
                "text",
                &[("r", "noop"), ("n", "disabled"), ("ctrl-t", "noop")],
            )]),
        ),
        (
            "host-key-taken-by-a-shared-action",
            cfg(&[("text", &[("r", "page_next"), ("/", "navigate:down")])]),
        ),
        (
            "host-op-on-an-inherited-key",
            cfg(&[
                ("text", &[("j", "refresh")]),
                ("image", &[("J", "mark_set")]),
            ]),
        ),
        (
            "host-op-on-a-menu-key",
            cfg(&[(
                "text",
                &[
                    ("y", "refresh"),
                    ("space", "mark_set"),
                    (",", "search_start"),
                ],
            )]),
        ),
        (
            "host-op-on-a-global-key",
            cfg(&[("text", &[("Q", "refresh"), ("t", "mark_set")])]),
        ),
        (
            "host-op-in-global",
            cfg(&[("global", &[("x", "mark_set"), ("r", "noop")])]),
        ),
        (
            "global-host-op-and-a-context-key",
            cfg(&[
                ("global", &[("x", "mark_set")]),
                ("text", &[("x", "page_next")]),
                ("table", &[("x", "refresh")]),
            ]),
        ),
        (
            "old-surface-names",
            cfg(&[
                ("preview_text", &[("x", "search_start"), ("r", "noop")]),
                ("preview_image", &[("x", "refresh")]),
                ("preview_table", &[("x", "mark_set")]),
            ]),
        ),
        (
            "old-name-beaten-by-new",
            cfg(&[
                ("preview_text", &[("x", "search_start")]),
                ("text", &[("x", "mark_set")]),
            ]),
        ),
        (
            "chord-of-a-host-op-and-a-typo",
            cfg(&[("text", &[("y x", "refresh"), ("w", "no_such_action")])]),
        ),
        (
            "canvas-and-image",
            cfg(&[
                ("canvas", &[("j", "refresh"), ("n", "search_next")]),
                ("image", &[("/", "search_start"), ("R", "noop")]),
            ]),
        ),
        (
            "non-ascii",
            cfg(&[(
                "text",
                &[("\u{3042}", "search_start"), ("\u{4e2d}", "noop")],
            )]),
        ),
    ]
}

#[test]
fn the_host_and_the_shared_tables_agree_on_the_host_operations_in_every_scenario() {
    for (label, c) in scenarios() {
        for scheme in [KeyScheme::Vim, KeyScheme::Less] {
            assert_agree(label, scheme, &c);
        }
    }
}

#[test]
fn the_comparison_covers_a_real_number_of_host_keys() {
    // A comparison that skips everything would pass.
    let n = assert_agree("defaults", KeyScheme::Vim, &cfg(&[]));
    assert!(n >= 45, "only {n} (context, key) pairs were compared");
    let n = assert_agree("every-name", KeyScheme::Vim, &scenarios()[2].1);
    assert!(n >= 45, "only {n} (context, key) pairs were compared");
}

#[test]
fn every_host_operation_has_the_name_konoma_gives_its_action() {
    for o in HostOp::ALL {
        if let Some(a) = host_op_action(o) {
            assert_eq!(action_name(a), o.name(), "{o:?}");
            assert_eq!(action_from_str(o.name()), Some(a), "{}", o.name());
            assert_eq!(a.core(), None, "{} is not a core action", o.name());
        }
    }
    assert_eq!(
        action_from_str("toggle_markdown_raw"),
        host_op_action(HostOp::CycleRepresentation)
    );
}

#[test]
fn every_mnemonic_names_an_action_of_the_host_or_of_a_build_without_it() {
    use konoma_host_core::keymap::{mnemonic_names, mnemonic_of_name};
    for n in mnemonic_names() {
        let known = action_from_str(n).is_some();
        let git_only = matches!(
            n,
            "toggle_follow_diff_scope" | "git_open_log" | "git_open_graph"
        );
        assert!(
            known || (git_only && !cfg!(feature = "git")),
            "`{n}` is not an action name"
        );
        if let Some(a) = action_from_str(n) {
            assert_eq!(action_name(a), n);
            assert_eq!(mnemonic_of_name(&action_name(a)), mnemonic_of_name(n));
        }
    }
}
