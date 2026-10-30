//! The `Space` menu over a preview, in the konoma host and in the standalone app (`konoma-plugin`
//! feature `app`). The promise: the operations the app has are in its menu under the keys, labels
//! and order konoma gives them, an item runs what its key would, and a key that is not an item
//! only closes the menu.
//!
//! The host's menu holds more (tabs, search, bookmarks, the editor, ...): those keys do nothing in
//! the app, so the menu does not name them. `HOST_ONLY` lists them by operation, and every entry
//! must occur: a new item in the host forces a decision here. The letters of the items that have no
//! key of their own (`back`, `path`, `wrap`) run along the whole menu, host-only items included:
//! the app builds the same candidates (the host's operations take their letters and are hidden),
//! so the letters are the host's. The one exception is a document, which is the text context in
//! the app.

use super::plugin_act_parity::*;
use super::table_parity::{panel_lines, APP_H, HOST_H};
use crate::keymap::{KeyPress, LeaderId};
use crate::ops::menu::{menu, View};
use crossterm::event::KeyCode;
use std::collections::BTreeSet;

/// The host's operations that have no counterpart in the app, by name (`{:?}` of the action).
const HOST_ONLY: &[&str] = &[
    "PreviewFileNext",
    "PreviewFilePrev",
    "RequestEdit",
    "OpenExternal",
    "TabPrev",
    "TabNext",
    "MarkSet",
    "MarkJump",
    "Refresh",
    "PreviewCopySelectionRef",
    "TabNewAfter",
    "ToggleTabList",
    "PasteJump",
    "ToggleFollow",
    "Quit",
    "SearchStart",
    "SearchNext",
    "SearchPrev",
    "PreviewEnterVisual",
    "PreviewEnterVisualLine",
    "LinkFocusNext",
    "LinkFocusPrev",
    "ToggleOutline",
];

/// The operations both sides have, by name; every plugin action is one too.
const SHARED: &[&str] = &[
    "PreviewBack",
    "ToggleHelp",
    "CyclePathStyle",
    "ToggleWrap",
    "ToggleTableCell",
];

#[derive(Debug, Clone, PartialEq)]
struct Item {
    key: char,
    label: String,
    action: String,
}

/// The menu the host builds for the file on screen: every item, with the operation it runs.
fn host_items(root: &std::path::Path, o: &Opt) -> Vec<Item> {
    let (s, _log, _opens) = host_sim_sized(root, o, HOST_H);
    let ctx = s.app.key_context().expect("a preview context");
    let m = menu(&s.app, ctx, LeaderId::Actions, View::Open).expect("the Space menu");
    m.items
        .into_iter()
        .map(|i| {
            let KeyCode::Char(key) = i.key.code else {
                panic!("a character key: {:?}", i.key)
            };
            Item {
                key,
                label: i.label,
                action: format!("{:?}", i.action),
            }
        })
        .collect()
}

/// The items of a menu line (`Actions ▸ k:label  k:label ...`); `None` when no menu is open.
fn line_items(rows: &[String]) -> Option<Vec<(char, String)>> {
    let last = rows.last()?;
    let (head, rest) = last.split_once("▸ ")?;
    if !(head.trim() == "Actions" || head.trim() == "操作") {
        return None;
    }
    let rest = rest.trim_end().trim_end_matches('…');
    Some(
        rest.split("  ")
            .filter_map(|t| {
                let (k, l) = t.trim().split_once(':')?;
                let mut cs = k.chars();
                let c = cs.next()?;
                cs.next().is_none().then(|| (c, l.to_string()))
            })
            .collect(),
    )
}

/// A key step for a letter found at run time (leaked: a test-only string of one character).
fn kc(c: char) -> Step {
    K(Box::leak(c.to_string().into_boxed_str()))
}

fn is_shared(it: &Item) -> bool {
    it.action.starts_with("Plugin(") || SHARED.contains(&it.action.as_str())
}

/// Whether the item runs on a key of its own (the plugin's declared key or `?`), so its key is the
/// same on both sides.
fn keyed(it: &Item) -> bool {
    matches!(it.key, 'x' | 'w' | 'u' | 'B' | '?')
}

fn open_run_app(root: &std::path::Path, o: &Opt, more: &[Step]) -> Run {
    let mut steps = vec![K("<Sp>")];
    steps.extend(more.iter().cloned());
    // The sentinel key would close an open menu: only a run that goes on past the menu has one.
    app(root, o, APP_H, &steps, !more.is_empty())
}

fn open_run_host(root: &std::path::Path, o: &Opt, more: &[Step]) -> Run {
    let mut steps = vec![K("<Sp>")];
    steps.extend(more.iter().cloned());
    host(root, o, HOST_H, &steps)
}

#[test]
fn the_menu_lists_what_the_app_has_under_konomas_keys_labels_and_order() {
    let (_dir, root) = fixture();
    let mut compared = 0;
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut shared_seen: BTreeSet<String> = BTreeSet::new();
    for kind in KINDS {
        for lang in ["en", "ja"] {
            let o = Opt::new(kind).lang(lang);
            let all = host_items(&root, &o);
            let want: Vec<&Item> = all.iter().filter(|i| is_shared(i)).collect();
            for it in &all {
                if is_shared(it) {
                    shared_seen.insert(it.action.split('{').next().unwrap().trim().to_string());
                } else {
                    assert!(
                        HOST_ONLY.contains(&it.action.as_str()),
                        "{kind:?} {lang}: the host item {it:?} is neither shared nor listed as host-only"
                    );
                    seen.insert(it.action.clone());
                }
            }
            // The host really opens that menu.
            let hr = open_run_host(&root, &o, &[]);
            let hline = line_items(&hr.rows).expect("the host's menu line");
            for (n, (k, l)) in hline.iter().enumerate() {
                assert_eq!(
                    (*k, l),
                    (all[n].key, &all[n].label),
                    "{kind:?} {lang}: host line"
                );
            }
            let ar = open_run_app(&root, &o, &[]);
            let aline = line_items(&ar.rows)
                .unwrap_or_else(|| panic!("{kind:?} {lang}: the app's menu line: {:?}", ar.rows));
            assert_eq!(
                aline.iter().map(|(_, l)| l.as_str()).collect::<Vec<_>>(),
                want.iter().map(|i| i.label.as_str()).collect::<Vec<_>>(),
                "{kind:?} {lang}: the labels, in the host's order"
            );
            let mut letters = BTreeSet::new();
            for ((k, l), w) in aline.iter().zip(&want) {
                // The letters are konoma's too: the app builds them from the same candidates and
                // the host's operations (hidden there) take theirs before it. A document is the
                // text context in the app (no focus over links, no outline), so its menu has the
                // host's text letters, not the document's: only its keyed items are compared.
                if kind != Kind::Document || keyed(w) {
                    assert_eq!(
                        *k,
                        w.key,
                        "{kind:?} {lang}: the key of {l} (keyed: {})",
                        keyed(w)
                    );
                }
                assert!(
                    keyed(w) || (k.is_alphanumeric() && *k != 'q'),
                    "{kind:?} {lang}: the letter of {l}: {k}"
                );
                assert!(letters.insert(*k), "{kind:?} {lang}: {k} is used twice");
                compared += 1;
            }
        }
    }
    // Not vacuous: the three declared actions, `back`, `help` and `path` at least, in each of the
    // eight runs.
    assert!(compared >= 8 * 6, "{compared}");
    let missing: Vec<&&str> = HOST_ONLY.iter().filter(|e| !seen.contains(**e)).collect();
    assert!(missing.is_empty(), "HOST_ONLY never occurs: {missing:?}");
    for want in [
        "Plugin(PluginAct",
        "PreviewBack",
        "ToggleHelp",
        "CyclePathStyle",
        "ToggleWrap",
        "ToggleTableCell",
    ] {
        assert!(
            shared_seen.iter().any(|s| s.starts_with(want)),
            "{want} never compared: {shared_seen:?}"
        );
    }
}

#[test]
fn an_item_runs_the_action_the_key_would_and_the_plugin_sees_the_same() {
    let (_dir, root) = fixture();
    let mut compared = 0;
    for kind in KINDS {
        for lang in ["en", "ja"] {
            let o = Opt::new(kind).lang(lang);
            for (steps, want_n, want_log) in [
                (vec![K("x")], "n=1", vec!["act:bump"]),
                (vec![K("w")], "n=100", vec!["act:save"]),
                (
                    vec![K("x"), K("w"), K("u")],
                    "n=1101",
                    vec!["act:bump", "act:save", "act:gated"],
                ),
            ] {
                // Each pick is its own `Space`.
                let mut seq: Vec<Step> = Vec::new();
                for s in &steps {
                    seq.push(K("<Sp>"));
                    seq.push(s.clone());
                }
                let hr = host(&root, &o, HOST_H, &seq);
                let ar = app(&root, &o, APP_H, &seq, true);
                assert!(!hr.left && !ar.left, "{kind:?} {lang} {steps:?}: left");
                assert_eq!(hr.log, want_log, "{kind:?} {lang} {steps:?}: host log");
                assert_eq!(ar.log, want_log, "{kind:?} {lang} {steps:?}: app log");
                let (hc, ac) = (counter(&hr.rows).unwrap(), counter(&ar.rows).unwrap());
                assert!(hc.starts_with(want_n), "{kind:?} {lang} {steps:?}: {hc}");
                assert_eq!(hc, ac, "{kind:?} {lang} {steps:?}");
                // The menu is closed after the pick.
                assert!(line_items(&ar.rows).is_none() && line_items(&hr.rows).is_none());
                compared += 1;
            }
        }
    }
    assert_eq!(compared, KINDS.len() * 2 * 3);
}

#[test]
fn a_key_that_is_not_an_item_only_closes_the_menu() {
    let (_dir, root) = fixture();
    let mut compared = 0;
    for kind in KINDS {
        let o = Opt::new(kind);
        for close in ["<Esc>", "q", "z", "<Sp>"] {
            let hr = open_run_host(&root, &o, &[K(close)]);
            let ar = open_run_app(&root, &o, &[K(close)]);
            for (who, r) in [("host", &hr), ("app", &ar)] {
                assert!(!r.left, "{kind:?} {close} {who}: left");
                assert!(r.log.is_empty(), "{kind:?} {close} {who}: {:?}", r.log);
                assert!(
                    line_items(&r.rows).is_none(),
                    "{kind:?} {close} {who}: the menu is still open"
                );
                assert!(counter(&r.rows).unwrap().starts_with("n=0 "), "{who}");
            }
            compared += 1;
        }
    }
    assert_eq!(compared, KINDS.len() * 4);
}

#[test]
fn an_action_that_does_not_act_or_may_not_run_is_not_in_the_menu() {
    let (_dir, root) = fixture();
    let mut compared = 0;
    for kind in KINDS {
        for (o, gone, stays) in [
            (Opt::new(kind).gate(false), "gated", "bump"),
            (
                Opt::new(kind).toml("[plugins.\"{id}\"]\nread_only = true\n"),
                "save",
                "bump",
            ),
        ] {
            let hall = host_items(&root, &o);
            assert!(
                !hall.iter().any(|i| i.label == gone),
                "{kind:?} host {gone}"
            );
            assert!(
                hall.iter().any(|i| i.label == stays),
                "{kind:?} host {stays}"
            );
            let ar = open_run_app(&root, &o, &[]);
            let aline = line_items(&ar.rows).expect("the app's menu line");
            assert!(
                !aline.iter().any(|(_, l)| l == gone),
                "{kind:?} app {gone}: {aline:?}"
            );
            assert!(
                aline.iter().any(|(_, l)| l == stays),
                "{kind:?} app {stays}: {aline:?}"
            );
            // The key of the missing item is no item: it closes the menu and runs nothing.
            let key = if gone == "gated" { "u" } else { "w" };
            let hr = open_run_host(&root, &o, &[K(key)]);
            let ar = open_run_app(&root, &o, &[K(key)]);
            for (who, r) in [("host", &hr), ("app", &ar)] {
                assert!(r.log.is_empty(), "{kind:?} {gone} {who}: {:?}", r.log);
                assert!(line_items(&r.rows).is_none(), "{kind:?} {gone} {who}");
            }
            compared += 1;
        }
    }
    assert_eq!(compared, KINDS.len() * 2);
}

#[test]
fn back_and_help_from_the_menu_do_what_their_keys_do() {
    let (_dir, root) = fixture();
    let mut compared = 0;
    for kind in KINDS {
        let o = Opt::new(kind);
        let all = host_items(&root, &o);
        let letter_of = |action: &str| all.iter().find(|i| i.action == action).unwrap().key;
        let hback = letter_of("PreviewBack");
        let ar0 = open_run_app(&root, &o, &[]);
        let aback = line_items(&ar0.rows)
            .unwrap()
            .into_iter()
            .find(|(_, l)| l == "back")
            .expect("back in the app's menu")
            .0;
        let hr = open_run_host(&root, &o, &[kc(hback)]);
        let ar = open_run_app(&root, &o, &[kc(aback)]);
        assert!(
            hr.left && ar.left,
            "{kind:?}: back leaves ({} {})",
            hr.left,
            ar.left
        );
        // `?` opens the help on both, the same help.
        let hr = host(&root, &o, 130, &[K("<Sp>"), K("?")]);
        let ar = app(&root, &o, 129, &[K("<Sp>"), K("?")], true);
        let (hl, al) = (panel_lines(&hr.rows), panel_lines(&ar.rows));
        assert_eq!(hl.first(), al.first(), "{kind:?}: the help's first line");
        compared += 1;
    }
    assert_eq!(compared, KINDS.len());
}

#[test]
fn the_display_options_and_the_cell_popup_run_from_the_menu() {
    let (_dir, root) = fixture();
    let mut compared = 0;
    let title = |rows: &[String]| {
        rows.iter()
            .find(|r| r.contains("a.pnt") && r.contains('┌'))
            .map(|r| r.trim_end().to_string())
    };
    for kind in KINDS {
        let o = Opt::new(kind);
        let all = host_items(&root, &o);
        let hkey = all
            .iter()
            .find(|i| i.action == "CyclePathStyle")
            .unwrap()
            .key;
        let ar0 = open_run_app(&root, &o, &[]);
        let akey = line_items(&ar0.rows)
            .unwrap()
            .into_iter()
            .find(|(_, l)| l == "path")
            .expect("path in the app's menu")
            .0;
        let h0 = host(&root, &o, HOST_H, &[]);
        let a0 = app(&root, &o, APP_H, &[], true);
        // A picture has no frame in the app, so the style shows nowhere there.
        let framed = kind != Kind::Canvas;
        if framed {
            assert_eq!(
                title(&h0.rows),
                title(&a0.rows),
                "{kind:?}: the title at the start"
            );
        }
        let hr = open_run_host(&root, &o, &[kc(hkey)]);
        let ar = open_run_app(&root, &o, &[kc(akey)]);
        assert_ne!(
            title(&hr.rows),
            title(&h0.rows),
            "{kind:?}: host path style"
        );
        if framed {
            assert_ne!(title(&ar.rows), title(&a0.rows), "{kind:?}: app path style");
            assert_eq!(
                title(&hr.rows),
                title(&ar.rows),
                "{kind:?}: the title after `path`"
            );
        }
        compared += 1;
    }
    // The table's cell popup.
    let o = Opt::new(Kind::Table);
    let all = host_items(&root, &o);
    let hkey = all
        .iter()
        .find(|i| i.action == "ToggleTableCell")
        .unwrap()
        .key;
    let ar0 = open_run_app(&root, &o, &[]);
    let akey = line_items(&ar0.rows)
        .unwrap()
        .into_iter()
        .find(|(_, l)| l == "view cell")
        .expect("view cell in the app's menu")
        .0;
    let h0 = host(&root, &o, HOST_H, &[]);
    let a0 = app(&root, &o, APP_H, &[], true);
    let hr = open_run_host(&root, &o, &[kc(hkey)]);
    let ar = open_run_app(&root, &o, &[kc(akey)]);
    assert_ne!(hr.rows, h0.rows, "the host's popup");
    assert_ne!(ar.rows, a0.rows, "the app's popup");
    compared += 1;
    assert_eq!(compared, KINDS.len() + 1);
}

#[test]
fn after_a_reopen_the_menu_lists_the_destinations_actions() {
    let (_dir, root) = fixture();
    let mut compared = 0;
    for lang in ["en", "ja"] {
        let o = Opt::new(Kind::Document).lang(lang).reopen();
        let all = host_items(&root, &o);
        let want: Vec<&Item> = all.iter().filter(|i| is_shared(i)).collect();
        let ar = open_run_app(&root, &o, &[]);
        let aline = line_items(&ar.rows).expect("the app's menu line");
        assert_eq!(
            aline.iter().map(|(_, l)| l.as_str()).collect::<Vec<_>>(),
            want.iter().map(|i| i.label.as_str()).collect::<Vec<_>>(),
            "{lang}"
        );
        let mine = if lang == "ja" {
            "対象加算"
        } else {
            "tbump"
        };
        assert!(
            aline.iter().any(|(k, l)| *k == 'B' && l == mine),
            "{lang}: {aline:?}"
        );
        // The original's actions are gone, and a pick runs the destination's.
        for gone in labels(lang, true) {
            let l = gone.split_once(':').unwrap().1.to_string();
            assert!(
                !aline.iter().any(|(_, x)| *x == l),
                "{lang}: {l} is still there"
            );
        }
        let hr = open_run_host(&root, &o, &[K("B")]);
        let ar = open_run_app(&root, &o, &[K("B")]);
        assert_eq!(hr.log, ["tact:tbump"], "{lang} host");
        assert_eq!(ar.log, ["tact:tbump"], "{lang} app");
        assert_eq!(counter(&hr.rows), counter(&ar.rows), "{lang}");
        // The original's key does nothing.
        let ar = open_run_app(&root, &o, &[K("x")]);
        assert!(ar.log.is_empty(), "{lang}: {:?}", ar.log);
        compared += 1;
    }
    assert_eq!(compared, 2);
}

#[allow(dead_code)]
fn _uses(_: KeyPress) {}
