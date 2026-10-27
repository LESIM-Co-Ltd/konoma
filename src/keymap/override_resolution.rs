//! How `[keys]` overrides resolve (plugin stage 3, step 0). Each scenario is a `[keys]` table as a
//! user would write it (new context names, the global table, the old `preview_*` names and the
//! old `copy_*` settings); it is built with `KeyMap::from_config` the way `App` builds it, and
//! every key a person can press is resolved in the contexts text, document, table, image, canvas
//! and tree, plain and behind each menu key. The literal lists what *differs* from the default
//! keymap (`defaults_snapshot` pins the defaults themselves), so an override that spills into a
//! context it was not written for, or a key that stops resolving, changes the text. Regenerate on
//! purpose with `KONOMA_PRINT_SNAPSHOTS=1 cargo nextest run -p konoma override_resolution
//! --no-capture` and read the output before pasting it in.

use super::*;
use crate::config::KeysConfig;

const CONTEXTS: [Context; 6] = [
    Context::Text,
    Context::Document,
    Context::Table,
    Context::Image,
    Context::Canvas,
    Context::Tree,
];

const MENUS: [LeaderId; 5] = [
    LeaderId::Copy,
    LeaderId::File,
    LeaderId::TableCopy,
    LeaderId::Options,
    LeaderId::Actions,
];

/// Every key a person can press: printable ASCII (the space is `Space`), Ctrl + a letter, and the
/// named keys.
fn probes() -> Vec<KeyPress> {
    let mut v: Vec<KeyPress> = (0x20u8..0x7f)
        .map(|b| KeyPress::ch(char::from(b)))
        .collect();
    v.extend(('a'..='z').map(KeyPress::ctrl_ch));
    for code in [
        KeyCode::Enter,
        KeyCode::Tab,
        KeyCode::BackTab,
        KeyCode::Esc,
        KeyCode::Up,
        KeyCode::Down,
        KeyCode::Left,
        KeyCode::Right,
        KeyCode::PageUp,
        KeyCode::PageDown,
        KeyCode::Home,
        KeyCode::End,
        KeyCode::Backspace,
        KeyCode::Delete,
    ] {
        v.push(KeyPress::key(code));
    }
    v
}

/// The `[keys]` of one scenario: `(table, [(key, action)])`.
type Tables<'a> = &'a [(&'a str, &'a [(&'a str, &'a str)])];

fn keys_config(tables: Tables<'_>) -> KeysConfig {
    let mut k = KeysConfig::default();
    for (table, entries) in tables {
        let t = k.surfaces.entry((*table).to_string()).or_default();
        for (key, action) in *entries {
            t.insert((*key).to_string(), (*action).to_string());
        }
    }
    k
}

/// What `scenario` changes against the default keymap, one line per context and key, then the
/// warnings, the conflicts (a key a context may not take from `global`) and the renamed names it
/// caused.
fn resolution_diff(scheme: KeyScheme, keys: &KeysConfig) -> String {
    let base = KeyMap::defaults(scheme);
    let map = KeyMap::from_config(scheme, &keys.to_keymap_config());
    let mut out = String::new();
    for ctx in CONTEXTS {
        for kp in probes() {
            for pending in std::iter::once(None).chain(MENUS.into_iter().map(Some)) {
                let (a, b) = (
                    base.resolve(ctx, pending, kp),
                    map.resolve(ctx, pending, kp),
                );
                if format!("{a:?}") != format!("{b:?}") {
                    let via = pending.map(|l| format!("{l:?} ")).unwrap_or_default();
                    out.push_str(&format!(
                        "{} | {via}{} | {a:?} -> {b:?}\n",
                        ctx.name(),
                        kp.label()
                    ));
                }
            }
        }
    }
    for w in &map.warnings {
        out.push_str(&format!("warning: {w}\n"));
    }
    for c in &map.conflicts {
        out.push_str(&format!(
            "conflict: {:?} {} {:?}: kept {:?}, dropped {:?}\n",
            c.surface,
            c.key.label(),
            c.reason,
            c.kept,
            c.dropped
        ));
    }
    for (old, now) in &map.renames {
        out.push_str(&format!("renamed: {old} -> {now}\n"));
    }
    out
}

/// Compare with the literal; with `KONOMA_PRINT_SNAPSHOTS` set, print the table instead.
#[track_caller]
fn check(name: &str, got: &str, want: &str) {
    if std::env::var_os("KONOMA_PRINT_SNAPSHOTS").is_some() {
        println!("=====BEGIN {name}=====\n{got}=====END {name}=====");
        return;
    }
    assert_eq!(got, want, "{name} changed");
}

macro_rules! scenario {
    ($test:ident, $scheme:expr, $tables:expr, $legacy:expr, $want:expr) => {
        #[test]
        fn $test() {
            let mut keys = keys_config($tables);
            #[allow(clippy::redundant_closure_call)]
            ($legacy)(&mut keys);
            check(stringify!($test), &resolution_diff($scheme, &keys), $want);
        }
    };
}

fn no_legacy(_: &mut KeysConfig) {}

scenario!(
    override_resolution_text_table,
    KeyScheme::Vim,
    &[(
        "text",
        &[
            ("z", "toggle_wrap"),
            ("j", "toggle_help"),
            ("q", "noop"),
            ("space z", "refresh"),
            ("y z", "copy_name"),
            (", z", "toggle_hidden")
        ]
    )],
    no_legacy,
    r####"text | j | Action(Navigate(Down)) -> Action(ToggleHelp)
text | q | Action(PreviewBack) -> Unbound
text | z | Unbound -> Action(ToggleWrap)
text | Copy z | Unbound -> Action(CopyPath(Name))
text | Options z | Unbound -> Action(ToggleHidden)
warning: the Space menu of [keys.text] lists its operations itself (bind the operation to a key there instead): "space z"
"####
);
scenario!(
    override_resolution_document_table,
    KeyScheme::Vim,
    &[(
        "document",
        &[
            ("z", "toggle_wrap"),
            ("ctrl-n", "toggle_help"),
            ("o", "noop"),
            ("Tab", "refresh"),
            ("space z", "refresh")
        ]
    )],
    no_legacy,
    r####"document | o | Action(OpenExternal) -> Unbound
document | z | Unbound -> Action(ToggleWrap)
document | Ctrl-n | Action(PreviewFileNext) -> Action(ToggleHelp)
warning: cannot rebind fixed key: "Tab"
warning: the Space menu of [keys.document] lists its operations itself (bind the operation to a key there instead): "space z"
"####
);
scenario!(
    override_resolution_table_table,
    KeyScheme::Vim,
    &[(
        "table",
        &[
            ("z", "toggle_table_cell"),
            ("l", "navigate:down"),
            ("y x", "table_copy_row"),
            ("y c", "noop"),
            ("enter", "refresh")
        ]
    )],
    no_legacy,
    r####"table | TableCopy c | Action(TableCopy(Cell)) -> Unbound
table | l | Action(Navigate(Right)) -> Action(Navigate(Down))
table | TableCopy x | Unbound -> Action(TableCopy(Row))
table | z | Unbound -> Action(ToggleTableCell)
warning: cannot rebind fixed key: "enter"
"####
);
scenario!(
    override_resolution_image_table,
    KeyScheme::Vim,
    &[(
        "image",
        &[
            ("z", "image_zoom_reset"),
            ("+", "image_zoom_out"),
            ("q", "noop"),
            ("space z", "refresh")
        ]
    )],
    no_legacy,
    r####"image | + | Action(ImageZoomIn) -> Action(ImageZoomOut)
image | q | Action(PreviewBack) -> Unbound
image | z | Unbound -> Action(ImageZoomReset)
warning: the Space menu of [keys.image] lists its operations itself (bind the operation to a key there instead): "space z"
"####
);
scenario!(
    override_resolution_canvas_table,
    KeyScheme::Vim,
    &[(
        "canvas",
        &[
            ("z", "refresh"),
            ("e", "open_external"),
            ("R", "noop"),
            ("y z", "copy_name")
        ]
    )],
    no_legacy,
    r####"canvas | R | Action(CycleRepresentation) -> Unbound
canvas | e | Action(RequestEdit) -> Action(OpenExternal)
canvas | z | Unbound -> Action(Refresh)
canvas | Copy z | Unbound -> Action(CopyPath(Name))
"####
);
scenario!(
    override_resolution_global_table,
    KeyScheme::Vim,
    &[(
        "global",
        &[
            ("z", "toggle_help"),
            ("Q", "noop"),
            ("ctrl-q", "quit"),
            ("space z", "refresh"),
            ("y z", "copy_name")
        ]
    )],
    no_legacy,
    r####"text | Q | Action(Quit) -> Unbound
text | z | Unbound -> Action(ToggleHelp)
text | Copy z | Unbound -> Action(CopyPath(Name))
text | File z | Unbound -> Action(Refresh)
text | Ctrl-q | Unbound -> Action(Quit)
document | Q | Action(Quit) -> Unbound
document | z | Unbound -> Action(ToggleHelp)
document | Copy z | Unbound -> Action(CopyPath(Name))
document | File z | Unbound -> Action(Refresh)
document | Ctrl-q | Unbound -> Action(Quit)
table | Q | Action(Quit) -> Unbound
table | z | Unbound -> Action(ToggleHelp)
table | Copy z | Unbound -> Action(CopyPath(Name))
table | File z | Unbound -> Action(Refresh)
table | Ctrl-q | Unbound -> Action(Quit)
image | Q | Action(Quit) -> Unbound
image | z | Unbound -> Action(ToggleHelp)
image | Copy z | Unbound -> Action(CopyPath(Name))
image | File z | Unbound -> Action(Refresh)
image | Ctrl-q | Unbound -> Action(Quit)
canvas | Q | Action(Quit) -> Unbound
canvas | z | Unbound -> Action(ToggleHelp)
canvas | Copy z | Unbound -> Action(CopyPath(Name))
canvas | File z | Unbound -> Action(Refresh)
canvas | Ctrl-q | Unbound -> Action(Quit)
tree | Q | Action(Quit) -> Unbound
tree | z | Unbound -> Action(ToggleHelp)
tree | Copy z | Unbound -> Action(CopyPath(Name))
tree | File z | Unbound -> Action(Refresh)
tree | Ctrl-q | Unbound -> Action(Quit)
"####
);
scenario!(
    override_resolution_global_and_context_on_one_key,
    KeyScheme::Vim,
    &[
        ("global", &[("z", "toggle_help")]),
        ("text", &[("z", "refresh")]),
        ("image", &[("z", "noop")])
    ],
    no_legacy,
    r####"text | z | Unbound -> Action(ToggleHelp)
document | z | Unbound -> Action(ToggleHelp)
table | z | Unbound -> Action(ToggleHelp)
image | z | Unbound -> Action(ToggleHelp)
canvas | z | Unbound -> Action(ToggleHelp)
tree | z | Unbound -> Action(ToggleHelp)
conflict: PreviewText z GlobalShadow: kept "toggle_help", dropped "refresh in [keys.text]"
"####
);
scenario!(
    override_resolution_old_copy_keys,
    KeyScheme::Vim,
    &[],
    |k: &mut KeysConfig| {
        k.copy_name = "N".into();
        k.copy_full = "F".into();
        k.copy_relative = "R".into();
        k.copy_parent = "D".into();
    },
    r####"text | Copy D | Unbound -> Action(CopyPath(Parent))
text | Copy F | Unbound -> Action(CopyPath(Full))
text | Copy N | Unbound -> Action(CopyPath(Name))
text | Copy R | Unbound -> Action(CopyPath(Relative))
document | Copy D | Unbound -> Action(CopyPath(Parent))
document | Copy F | Unbound -> Action(CopyPath(Full))
document | Copy N | Unbound -> Action(CopyPath(Name))
document | Copy R | Unbound -> Action(CopyPath(Relative))
table | Copy D | Unbound -> Action(CopyPath(Parent))
table | Copy F | Unbound -> Action(CopyPath(Full))
table | Copy N | Unbound -> Action(CopyPath(Name))
table | Copy R | Unbound -> Action(CopyPath(Relative))
image | Copy D | Unbound -> Action(CopyPath(Parent))
image | Copy F | Unbound -> Action(CopyPath(Full))
image | Copy N | Unbound -> Action(CopyPath(Name))
image | Copy R | Unbound -> Action(CopyPath(Relative))
canvas | Copy D | Unbound -> Action(CopyPath(Parent))
canvas | Copy F | Unbound -> Action(CopyPath(Full))
canvas | Copy N | Unbound -> Action(CopyPath(Name))
canvas | Copy R | Unbound -> Action(CopyPath(Relative))
tree | Copy D | Unbound -> Action(CopyPath(Parent))
tree | Copy F | Unbound -> Action(CopyPath(Full))
tree | Copy N | Unbound -> Action(CopyPath(Name))
tree | Copy R | Unbound -> Action(CopyPath(Relative))
"####
);
scenario!(
    override_resolution_old_copy_keys_left_at_the_default_change_nothing,
    KeyScheme::Vim,
    &[],
    |k: &mut KeysConfig| {
        k.copy_prefix = "c".into();
    },
    r####""####
);
scenario!(
    override_resolution_old_copy_keys_do_not_clobber_a_written_chord,
    KeyScheme::Vim,
    &[("global", &[("y N", "copy_full")])],
    |k: &mut KeysConfig| {
        k.copy_name = "N".into();
    },
    r####"text | Copy N | Unbound -> Action(CopyPath(Full))
document | Copy N | Unbound -> Action(CopyPath(Full))
table | Copy N | Unbound -> Action(CopyPath(Full))
image | Copy N | Unbound -> Action(CopyPath(Full))
canvas | Copy N | Unbound -> Action(CopyPath(Full))
tree | Copy N | Unbound -> Action(CopyPath(Full))
"####
);
scenario!(
    override_resolution_old_context_names,
    KeyScheme::Vim,
    &[
        ("preview_text", &[("z", "toggle_wrap")]),
        ("preview_image", &[("z", "image_zoom_reset")]),
        ("preview_table", &[("z", "toggle_table_cell")])
    ],
    no_legacy,
    r####"text | z | Unbound -> Action(ToggleWrap)
document | z | Unbound -> Action(ToggleWrap)
table | z | Unbound -> Action(ToggleTableCell)
image | z | Unbound -> Action(ImageZoomReset)
renamed: [keys.preview_image] -> [keys.image]
renamed: [keys.preview_table] -> [keys.table]
renamed: [keys.preview_text] -> [keys.text] + [keys.document]
"####
);
scenario!(
    override_resolution_new_name_beats_old_name,
    KeyScheme::Vim,
    &[
        ("preview_text", &[("z", "toggle_wrap")]),
        ("text", &[("z", "refresh")])
    ],
    no_legacy,
    r####"text | z | Unbound -> Action(Refresh)
document | z | Unbound -> Action(ToggleWrap)
renamed: [keys.preview_text] -> [keys.text] + [keys.document]
"####
);
scenario!(
    override_resolution_refused_entries_change_nothing,
    KeyScheme::Vim,
    &[
        ("text", &[("z", "no_such_action"), ("1", "refresh")]),
        ("table", &[("2", "tab_goto"), ("x y z", "refresh")]),
        ("image", &[("q q", "refresh")]),
        ("nowhere", &[("z", "refresh")])
    ],
    no_legacy,
    r####"warning: unknown key surface: nowhere
warning: unsupported chord prefix (only `space`, `y` and `,` open menus): "q q"
warning: tab_goto is fixed to digit keys and cannot be rebound
warning: invalid key "x y z": expected 1 or 2 key tokens, got 3: "x y z"
warning: unknown action: no_such_action
conflict: PreviewText 1 GlobalShadow: kept "tab_goto", dropped "refresh in [keys.text]"
"####
);
scenario!(
    override_resolution_less_scheme_text_table,
    KeyScheme::Less,
    &[("text", &[("z", "toggle_wrap"), ("space", "refresh")])],
    no_legacy,
    r####"text | z | Unbound -> Action(ToggleWrap)
conflict: PreviewText Space PrefixVsSingle: kept "leader:actions", dropped "refresh in [keys.text]"
"####
);
