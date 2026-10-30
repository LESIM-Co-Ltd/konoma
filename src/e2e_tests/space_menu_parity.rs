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
//! so the letters are the host's, a document's included (it is the document context in both).

use super::plugin_act_parity::*;
use super::table_parity::{hints, panel_box, panel_lines, rows_of, APP_H, HOST_H};
use crate::keymap::{action_name, Action, Context, KeyPress, LeaderId};
use crate::ops::menu::{menu, View};
use crossterm::event::{KeyCode, KeyModifiers};
use std::collections::BTreeSet;

/// The host's operations that have no counterpart in the app, by name (`{:?}` of the action).
const HOST_ONLY: &[&str] = &[
    "PreviewFileNext",
    "PreviewFilePrev",
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
    "ImageZoomIn",
    "ImageZoomOut",
    "ImageZoomReset",
    "RequestEdit",
    "OpenExternal",
];

#[derive(Debug, Clone, PartialEq)]
struct Item {
    key: char,
    label: String,
    action: String,
    /// The operation itself (for the `[keys]` rewrites made from the menu's own list).
    act: Action,
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
                act: i.action,
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
                // the host's operations (hidden there) take theirs before it, in the document
                // context as in any other.
                assert_eq!(
                    *k,
                    w.key,
                    "{kind:?} {lang}: the key of {l} (keyed: {})",
                    keyed(w)
                );
                assert!(
                    keyed(w)
                        // The picture's zoom items keep the keys of the zoom itself (`+`, `-`, ...).
                        || (w.action.starts_with("ImageZoom") && !k.is_whitespace())
                        || (k.is_alphanumeric() && *k != 'q'),
                    "{kind:?} {lang}: the letter of {l}: {k}"
                );
                assert!(letters.insert(*k), "{kind:?} {lang}: {k} is used twice");
                compared += 1;
            }
        }
    }
    // Not vacuous: the three declared actions, `back`, `help` and `path` at least, in each of the
    // ten runs.
    // (measured: 72, the picture's zoom items among them.)
    assert!(compared >= 72, "{compared}");
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
                let (hc, ac) = (hr.state.clone().unwrap(), ar.state.clone().unwrap());
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
                assert!(r.state.clone().unwrap().starts_with("n=0 "), "{who}");
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

// ---------------------------------------------------------------------------------------------
// The whole matrix: every view the app has x both languages x both key schemes x the user's
// `[keys]` rewrites, made from the menu's own list of operations.
// ---------------------------------------------------------------------------------------------

/// Every view the standalone app has: the four of [`KINDS`] and a still picture.
const VIEWS: [Kind; 5] = [
    Kind::Text,
    Kind::Document,
    Kind::Table,
    Kind::Image,
    Kind::Canvas,
];
const LANGS: [&str; 2] = ["en", "ja"];
const SCHEMES: [&str; 2] = ["vim", "less"];

/// What the host shows over the file: its key context, every item of the `Space` menu, the line
/// the open menu draws, the keys each operation has now, and every key the context binds.
struct HostView {
    ctx: Context,
    items: Vec<Item>,
    line: Vec<(char, String)>,
    keys: Vec<Vec<KeyPress>>,
    bound: Vec<KeyPress>,
    /// The menus `y` and `,` open on this screen: the prefix and the items by key and operation.
    menus: Vec<(char, Vec<(char, Action)>)>,
}

fn host_view(root: &std::path::Path, o: &Opt) -> HostView {
    let (mut s, _log, _opens) = host_sim_sized(root, o, HOST_H);
    let ctx = s.app.key_context().expect("a preview context");
    let m = menu(&s.app, ctx, LeaderId::Actions, View::Open).expect("the Space menu");
    let items: Vec<Item> = m
        .items
        .into_iter()
        .map(|i| {
            let KeyCode::Char(key) = i.key.code else {
                panic!("a character key: {:?}", i.key)
            };
            Item {
                key,
                label: i.label,
                action: format!("{:?}", i.action),
                act: i.action,
            }
        })
        .collect();
    let keys = items
        .iter()
        .map(|i| match i.act {
            Action::Plugin(_) => Vec::new(),
            a => s.app.keymaps.keys_of(ctx, a),
        })
        .collect();
    let mut bound: Vec<KeyPress> = s
        .app
        .keymaps
        .bound_actions(ctx)
        .into_iter()
        .map(|(k, _)| k)
        .collect();
    bound.extend(
        s.app
            .keymaps
            .leader_keys_of(ctx)
            .into_iter()
            .map(|(k, _)| k),
    );
    let copy_id = if ctx.name() == "table" {
        LeaderId::TableCopy
    } else {
        LeaderId::Copy
    };
    let menus: Vec<(char, Vec<(char, Action)>)> = [(',', LeaderId::Options), ('y', copy_id)]
        .into_iter()
        .filter_map(|(pre, id)| {
            let m = s.app.keymaps.menu(ctx, id)?;
            let its = m
                .items
                .iter()
                .filter_map(|it| match it.key.code {
                    KeyCode::Char(c) if !it.key.ctrl => Some((c, it.action)),
                    _ => None,
                })
                .collect();
            Some((pre, its))
        })
        .collect();
    s.press(KeyCode::Char(' '), KeyModifiers::NONE);
    s.term.clear().expect("clear");
    s.draw();
    let line = line_items(&rows_of(&s.term)).expect("the host's menu line");
    HostView {
        ctx,
        items,
        line,
        keys,
        bound,
        menus,
    }
}

/// A key as a config token (`None` for a key the rewrites do not write).
fn key_token(k: KeyPress) -> Option<String> {
    let base = match k.code {
        KeyCode::Char(' ') => "space".to_string(),
        KeyCode::Char(c) => c.to_string(),
        KeyCode::Tab => "tab".to_string(),
        KeyCode::Enter => "enter".to_string(),
        KeyCode::Esc => "esc".to_string(),
        _ => return None,
    };
    Some(if k.ctrl { format!("ctrl-{base}") } else { base })
}

fn toml_pair(key: &str, val: &str) -> String {
    format!(
        "\"{}\" = \"{val}\"\n",
        key.replace('\\', "\\\\").replace('"', "\\\"")
    )
}

/// One change of the user's `[keys]`, with a name that says what it did.
struct Rewrite {
    name: String,
    toml: String,
}

/// The rewrites made from the operations of the menu of `kind` (read from the host, in `scheme`):
/// for each operation, in each table that could reach it (its context's, `[keys.text]`,
/// `[keys.document]` and `[keys.global]`; a declared action's own `[keys."{id}"]`),
///
/// - take its keys away (`noop`),
/// - move it to a key nothing has,
/// - bind it to the letter another item of the menu would take (a letter free by default),
///
/// and, in each table, every operation's keys taken away at once, and every operation bound to
/// the next one's letter at once.
fn rewrites(root: &std::path::Path, kind: Kind, scheme: &str) -> Vec<Rewrite> {
    let o = Opt::new(kind).toml(&format!("keys = \"{scheme}\"\n"));
    let v = host_view(root, &o);
    let own = v.ctx.name();
    let mut tables: Vec<String> = Vec::new();
    for t in [own.as_str(), "text", "document", "global"] {
        if !tables.iter().any(|x| x == t) {
            tables.push(t.to_string());
        }
    }
    let free = ['Z', 'X', 'C', 'V', 'N', 'M']
        .into_iter()
        .find(|c| !v.bound.contains(&KeyPress::ch(*c)) && !v.items.iter().any(|i| i.key == *c))
        .expect("a free key");
    let letter_free = |c: char| c.is_alphanumeric() && !v.bound.contains(&KeyPress::ch(c));
    let n = v.items.len();
    // The letter of the next item (going round) that is free by default.
    let next_letter = |i: usize| {
        (1..n)
            .map(|d| v.items[(i + d) % n].key)
            .find(|c| letter_free(*c))
    };
    let mut out = Vec::new();
    for (i, it) in v.items.iter().enumerate() {
        let (name, cur, heads): (String, Vec<String>, Vec<String>) = match it.act {
            Action::Plugin(p) => (
                p.act().to_string(),
                vec![it.key.to_string()],
                vec!["keys.\"{id}\"".to_string()],
            ),
            a => (
                action_name(a),
                v.keys[i].iter().filter_map(|k| key_token(*k)).collect(),
                tables.iter().map(|t| format!("keys.{t}")).collect(),
            ),
        };
        for head in &heads {
            let tag = format!("{:?} {} in [{head}]", kind, it.action);
            let gone: String = cur.iter().map(|k| toml_pair(k, "noop")).collect();
            if !cur.is_empty() {
                out.push(Rewrite {
                    name: format!("{tag}: keys taken away"),
                    toml: format!("[{head}]\n{gone}"),
                });
                out.push(Rewrite {
                    name: format!("{tag}: moved to {free}"),
                    toml: format!("[{head}]\n{gone}{}", toml_pair(&free.to_string(), &name)),
                });
            }
            if let Some(l) = next_letter(i) {
                out.push(Rewrite {
                    name: format!("{tag}: bound to the letter {l}"),
                    toml: format!("[{head}]\n{}", toml_pair(&l.to_string(), &name)),
                });
            }
        }
    }
    // The user's edits of the menus `,` and `y` open (a chord `"<prefix> <key>"`): each item taken
    // away, moved to a free key, turned into another display operation; an item added; and a whole
    // menu emptied. The `Space` menu does not read these menus, so none of them may move it.
    let swap = |name: &str| match name {
        "cycle_path_style" => "toggle_wrap",
        _ => "cycle_path_style",
    };
    for head in &tables {
        let head = format!("keys.{head}");
        for (pre, its) in &v.menus {
            let tag = format!("{:?} the `{pre}` menu in [{head}]", kind);
            let mut emptied = String::new();
            for &(k, a) in its {
                let chord = format!("{pre} {k}");
                let name = action_name(a);
                emptied.push_str(&toml_pair(&chord, "noop"));
                out.push(Rewrite {
                    name: format!("{tag}: {chord} taken away"),
                    toml: format!("[{head}]\n{}", toml_pair(&chord, "noop")),
                });
                out.push(Rewrite {
                    name: format!("{tag}: {chord} moved to {free}"),
                    toml: format!(
                        "[{head}]\n{}{}",
                        toml_pair(&chord, "noop"),
                        toml_pair(&format!("{pre} {free}"), &name)
                    ),
                });
                if matches!(name.as_str(), "cycle_path_style" | "toggle_wrap") {
                    out.push(Rewrite {
                        name: format!("{tag}: {chord} turned into {}", swap(&name)),
                        toml: format!("[{head}]\n{}", toml_pair(&chord, swap(&name))),
                    });
                }
            }
            if !its.is_empty() {
                out.push(Rewrite {
                    name: format!("{tag}: emptied"),
                    toml: format!("[{head}]\n{emptied}"),
                });
            }
            for added in ["toggle_wrap", "cycle_path_style"] {
                out.push(Rewrite {
                    name: format!("{tag}: {added} added under {free}"),
                    toml: format!("[{head}]\n{}", toml_pair(&format!("{pre} {free}"), added)),
                });
            }
        }
    }
    // Every operation the shared tables name (each core action once per payload, and each host
    // operation), bound to a free key in each context's table, whether or not the operation acts
    // there: a gate that is closed in that context must hide it from the menu on both sides.
    let mut op_names: Vec<String> = konoma_host_core::keymap::CoreAction::all()
        .into_iter()
        .filter(|a| !matches!(a, konoma_host_core::keymap::CoreAction::TabGoto(_)))
        .map(|a| a.name())
        .collect();
    op_names.extend(
        konoma_host_core::keymap::HostOp::ALL
            .into_iter()
            .map(|o| o.name().to_string()),
    );
    op_names.sort();
    op_names.dedup();
    for ctx in ["text", "document", "table", "image", "canvas"] {
        for op in &op_names {
            out.push(Rewrite {
                name: format!("{kind:?} {op} assigned to {free} in [keys.{ctx}]"),
                toml: format!("[keys.{ctx}]\n{}", toml_pair(&free.to_string(), op)),
            });
        }
    }
    for t in &tables {
        let mut gone = String::new();
        let mut seen: Vec<String> = Vec::new();
        for (i, it) in v.items.iter().enumerate() {
            if matches!(it.act, Action::Plugin(_)) {
                continue;
            }
            for k in v.keys[i].iter().filter_map(|k| key_token(*k)) {
                if !seen.contains(&k) {
                    gone.push_str(&toml_pair(&k, "noop"));
                    seen.push(k);
                }
            }
        }
        out.push(Rewrite {
            name: format!("{kind:?} every key taken away in [keys.{t}]"),
            toml: format!("[keys.{t}]\n{gone}"),
        });
        let mut all = String::new();
        let mut used: Vec<char> = Vec::new();
        for (i, it) in v.items.iter().enumerate() {
            if matches!(it.act, Action::Plugin(_)) {
                continue;
            }
            if let Some(l) = next_letter(i).filter(|l| !used.contains(l)) {
                all.push_str(&toml_pair(&l.to_string(), &action_name(it.act)));
                used.push(l);
            }
        }
        out.push(Rewrite {
            name: format!("{kind:?} every operation bound to a letter in [keys.{t}]"),
            toml: format!("[keys.{t}]\n{all}"),
        });
    }
    out
}

/// The title of the help panel on a screen (the text in its top border).
fn panel_title(rows: &[String]) -> String {
    let (_, lines) = panel_box(rows);
    lines
        .first()
        .map(|l| {
            l.trim_matches(|c| c == '┌' || c == '┐' || c == '─')
                .trim()
                .to_string()
        })
        .unwrap_or_default()
}

/// What was compared over one matrix run.
#[derive(Default)]
struct Tally {
    variants: usize,
    /// The variants whose host menu differs from the same view without the rewrite.
    changed: usize,
    /// Items of the app's menu, compared by letter, label and place.
    items: usize,
    /// Items the host has and the app does not name (the host-only operations).
    hidden: usize,
    /// Help panels whose title and footer were compared.
    helps: usize,
    /// Variants where the app held a menu item under a letter the host's menu does not use.
    cells: BTreeSet<(usize, usize)>,
}

/// One view under one configuration: the app's menu is the host's, less what the app cannot do.
fn check(root: &std::path::Path, o: &Opt, tag: &str, t: &mut Tally) -> Vec<(char, String)> {
    let v = host_view(root, o);
    // The host's line is cut at the screen's width: what it draws is the start of its menu.
    let menu_line: Vec<(char, String)> = v.items.iter().map(|i| (i.key, i.label.clone())).collect();
    assert!(
        !v.line.is_empty()
            && v.line.len() <= menu_line.len()
            && v.line[..] == menu_line[..v.line.len()],
        "{tag}: the host's line is the start of its menu: {:?}",
        v.line
    );
    let want: Vec<&Item> = v.items.iter().filter(|i| is_shared(i)).collect();
    for it in v.items.iter().filter(|i| !is_shared(i)) {
        // An operation the user assigned to a key (the `assigned to` rewrites) can put any of the
        // host's operations in its menu; it is still not shared, so the app must not name it.
        assert!(
            HOST_ONLY.contains(&it.action.as_str()) || tag.contains(" assigned to "),
            "{tag}: the host item {it:?} is neither shared nor listed as host-only"
        );
        t.hidden += 1;
    }
    let ar = open_run_app(root, o, &[]);
    let aline =
        line_items(&ar.rows).unwrap_or_else(|| panic!("{tag}: the app's menu line: {:?}", ar.rows));
    assert_eq!(
        aline,
        want.iter()
            .map(|i| (i.key, i.label.clone()))
            .collect::<Vec<_>>(),
        "{tag}: the app's menu (letter, label, order)"
    );
    let mut letters = BTreeSet::new();
    for (k, l) in &aline {
        assert!(letters.insert(*k), "{tag}: {k} is used twice ({l})");
        t.items += 1;
    }
    // The help over that screen: the title and the footer follow the same keys.
    let help = v
        .items
        .iter()
        .find(|i| i.action == "ToggleHelp")
        .expect("the help in the menu")
        .key;
    let hr = host(root, o, HOST_H, &[K("<Sp>"), kc(help)]);
    let ar = app(root, o, APP_H, &[K("<Sp>"), kc(help)], true);
    assert_eq!(
        panel_title(&hr.rows),
        panel_title(&ar.rows),
        "{tag}: the help's title"
    );
    assert_eq!(
        hints(hr.rows.last().unwrap()),
        hints(ar.rows.last().unwrap()),
        "{tag}: the footer while the help is open"
    );
    t.helps += 1;
    t.variants += 1;
    v.line
}

fn matrix(kind: Kind) {
    let (_dir, root) = fixture();
    let mut t = Tally::default();
    let mut rewrites_n = 0;
    for (si, scheme) in SCHEMES.iter().enumerate() {
        let rws = rewrites(&root, kind, scheme);
        assert!(rws.len() >= 8, "{kind:?} {scheme}: {} rewrites", rws.len());
        rewrites_n += rws.len();
        for (li, lang) in LANGS.iter().enumerate() {
            let base = format!("keys = \"{scheme}\"\n");
            let plain = check(
                &root,
                &Opt::new(kind).lang(lang).toml(&base),
                &format!("{kind:?} {lang} {scheme} plain"),
                &mut t,
            );
            for rw in &rws {
                let o = Opt::new(kind)
                    .lang(lang)
                    .toml(&format!("{base}{}", rw.toml));
                let line = check(&root, &o, &format!("{lang} {scheme} {}", rw.name), &mut t);
                if line != plain {
                    t.changed += 1;
                }
                t.cells.insert((si, li));
            }
        }
    }
    eprintln!(
        "matrix {kind:?}: rewrites {rewrites_n} variants {} changed {} items {} hidden {} helps {}",
        t.variants, t.changed, t.items, t.hidden, t.helps
    );
    // Every scheme x language cell ran.
    assert_eq!(t.cells.len(), SCHEMES.len() * LANGS.len(), "{kind:?}");
    // Not vacuous: (plain + rewrites) in each of the four cells, and the rewrites change the menu
    // in a good part of them (a rewrite that never shows would test nothing).
    assert_eq!(
        t.variants,
        LANGS.len() * SCHEMES.len() + LANGS.len() * rewrites_n,
        "{kind:?}"
    );
    // (measured per view: 244 to 348 changed, 1216 to 1306 rewrites; the floors sit below the smallest.)
    assert!(
        rewrites_n >= 1200,
        "{kind:?}: {rewrites_n} rewrites (measured 1216 to 1306)"
    );
    assert!(t.changed >= 240, "{kind:?} changed {}", t.changed);
    // At least back, the help and the three declared actions' worth (path, wrap, ...) per variant.
    // (measured: 6 to 9 items and 14 to 21 host-only items per variant, 14.01 at the least.)
    assert!(t.items >= t.variants * 6, "{kind:?} items {}", t.items);
    assert!(t.hidden >= t.variants * 10, "{kind:?} hidden {}", t.hidden);
    assert_eq!(t.helps, t.variants);
}

#[test]
fn the_matrix_text() {
    matrix(Kind::Text);
}
#[test]
fn the_matrix_document() {
    matrix(Kind::Document);
}
#[test]
fn the_matrix_table() {
    matrix(Kind::Table);
}
#[test]
fn the_matrix_image() {
    matrix(Kind::Image);
}
#[test]
fn the_matrix_canvas() {
    matrix(Kind::Canvas);
}

/// A view the app gains must get a matrix test: the list is every `Kind`, spelled out so that a
/// new variant does not compile until it is placed here.
#[test]
fn the_matrix_has_a_test_for_every_view() {
    for k in VIEWS {
        match k {
            Kind::Text | Kind::Document | Kind::Table | Kind::Image | Kind::Canvas => {}
        }
    }
    for k in KINDS {
        assert!(VIEWS.contains(&k), "{k:?}");
    }
    assert_eq!(VIEWS.len(), KINDS.len());
}

/// A key written only under `[keys.document]` acts in the app's decorated document, as in the
/// host: the keys are read for the Document context there, not the Text one. The help opening is
/// what shows it (it is shared, and a panel is on screen or it is not).
#[test]
fn a_key_written_under_keys_document_acts_in_the_apps_document() {
    let (_dir, root) = fixture();
    // The help's footer names how to close it; no other footer of a document does.
    let help_open = |rows: &[String]| rows.last().is_some_and(|r| r.contains(":close"));
    // `b` is a letter nothing in a document uses by default.
    let plain = Opt::new(Kind::Document);
    assert!(
        !help_open(&app(&root, &plain, APP_H, &[K("b")], true).rows),
        "b does nothing without the override"
    );
    let only_document = Opt::new(Kind::Document).toml("[keys.document]\n\"b\" = \"toggle_help\"\n");
    let hr = host(&root, &only_document, HOST_H, &[K("b")]);
    let ar = app(&root, &only_document, APP_H, &[K("b")], true);
    assert!(help_open(&hr.rows), "the host opens the help on b");
    assert!(help_open(&ar.rows), "the app opens the help on b");
    assert_eq!(panel_title(&hr.rows), panel_title(&ar.rows));
    // Written under `[keys.table]` it is for tables only: the document does not read it.
    let other = Opt::new(Kind::Document).toml("[keys.table]\n\"b\" = \"toggle_help\"\n");
    assert!(!help_open(&host(&root, &other, HOST_H, &[K("b")]).rows));
    assert!(!help_open(&app(&root, &other, APP_H, &[K("b")], true).rows));
}
