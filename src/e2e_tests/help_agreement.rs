//! The shared help and footer builders (`konoma_host_core::help`) read the keys through two
//! lookups: the host's (`HostLookup`, over the host keymap and its fixed keys) and the shared
//! tables' own (`CoreLookup`, what the standalone app uses). They must answer the same for every
//! context the app shows, in English and Japanese, with the defaults and with the user's keys
//! changed, and the shared sections must be what the host prints for the layers 1 to 3.

use konoma_host_core::help::{
    core_sections, footer_tokens, rows_of, CoreFoot, CoreLookup, CoreSection, HelpOp, KeyLookup,
};
use konoma_host_core::keymap::{scheme_from_str, CoreContext, CoreKeys, CoreMenu, KeyPress};

use super::*;
use crate::keymap::Context;
use crate::ops::core_help::{facts_for, HostLookup};

const CONTEXTS: [(Context, CoreContext); 5] = [
    (Context::Text, CoreContext::Text),
    (Context::Document, CoreContext::Document),
    (Context::Image, CoreContext::Image),
    (Context::Table, CoreContext::Table),
    (Context::Canvas, CoreContext::Canvas),
];

/// One key override: (config surface, key, action).
type Pair = (&'static str, &'static str, &'static str);

/// A key set: (name, `ui.keys`, overrides).
type Variant = (&'static str, &'static str, Vec<Pair>);

/// The key sets to try. `global` is layer 0; `image`/`table`/`text` are the surfaces of one
/// context, so a binding there can shadow a lower layer (and the global layer cannot be shadowed
/// by a different binding: `validate` restores it).
fn variants() -> Vec<Variant> {
    vec![
        ("default", "vim", vec![]),
        (
            "rebind text",
            "vim",
            vec![("text", "x", "toggle_help"), ("text", "H", "navigate:down")],
        ),
        ("unbind q", "vim", vec![("text", "q", "noop")]),
        (
            "unbind q and Q",
            "vim",
            vec![("text", "q", "noop"), ("text", "Q", "noop")],
        ),
        ("less", "less", vec![]),
        ("less and unbind", "less", vec![("text", "j", "noop")]),
        (
            "global rebind",
            "vim",
            vec![
                ("global", "w", "toggle_help"),
                ("global", "H", "navigate:down"),
            ],
        ),
        ("global unbind help", "vim", vec![("global", "?", "noop")]),
        (
            "image and table surfaces",
            "vim",
            vec![
                ("image", "z", "toggle_help"),
                ("image", "j", "noop"),
                ("table", "H", "navigate:down"),
                ("table", "q", "noop"),
            ],
        ),
        (
            "layer shadowing",
            "vim",
            vec![
                // A view-type / context binding over a basic-layer key, and a global binding.
                ("image", "J", "navigate:down"),
                ("table", "K", "navigate:up"),
                ("document", "g", "noop"),
                ("global", "w", "toggle_help"),
            ],
        ),
    ]
}

fn config(lang: &str, ui_keys: &str, pairs: &[Pair]) -> Config {
    let mut cfg = Config::default();
    cfg.ui.lang = lang.into();
    cfg.ui.keys = ui_keys.into();
    for (surface, k, a) in pairs {
        cfg.keys
            .surfaces
            .entry((*surface).to_string())
            .or_default()
            .insert((*k).to_string(), (*a).to_string());
    }
    cfg
}

fn sim(cfg: Config) -> (Sim, impl Sized) {
    let dir = sandbox("help_agreement");
    seed_files(&dir);
    let mut s = Sim::with_config(&canon(&dir), cfg);
    s.select("notes.txt");
    s.enter();
    (s, dir)
}

/// Every operation the shared rows and footer entries name, and the menu key.
fn all_ops(cctx: CoreContext) -> Vec<HelpOp> {
    let mut ops: Vec<HelpOp> = Vec::new();
    for row in rows_of(cctx) {
        ops.extend(row.ops().iter().copied());
    }
    ops.push(HelpOp::Menu(CoreMenu::Options));
    ops.push(HelpOp::Cancel);
    ops
}

/// `rows` appear in `host` in this order (the host lists its own rows between them).
fn is_subsequence(rows: &[(String, String)], host: &[(String, String)]) -> bool {
    let mut it = host.iter();
    rows.iter().all(|r| it.any(|h| h == r))
}

#[test]
fn the_two_lookups_agree_on_every_key_and_layer() {
    {
        for (name, ui_keys, pairs) in variants() {
            let cfg = config("en", ui_keys, &pairs);
            let core = CoreKeys::from_keys_config(scheme_from_str(&cfg.ui.keys), &cfg.keys);
            let (s, _dir) = sim(cfg);
            for (ctx, cctx) in CONTEXTS {
                let host = HostLookup { app: &s.app, ctx };
                let shared = CoreLookup {
                    keys: &core,
                    ctx: cctx,
                };
                let mut seen: Vec<KeyPress> = Vec::new();
                for op in all_ops(cctx) {
                    let (h, c) = (host.keys_of(op), shared.keys_of(op));
                    assert_eq!(h, c, "{name} {ctx:?}: keys of {op:?}");
                    seen.extend(h);
                }
                // Other keys: the host binds keys of its own (`m`, `y`...) that the shared tables
                // do not know, but a key the shared tables bind comes from the same layer.
                for ch in "abcdefghijklmnopqrstuvwxyzHJKLQ?0$+-=.,/ ".chars() {
                    seen.push(KeyPress::ch(ch));
                }
                let mut compared = 0;
                for kp in seen {
                    if let Some(layer) = shared.layer_of(kp) {
                        assert_eq!(
                            host.layer_of(kp),
                            Some(layer),
                            "{name} {ctx:?}: layer of {kp:?}"
                        );
                        compared += 1;
                    }
                }
                assert!(
                    compared >= 3,
                    "{name} {ctx:?}: only {compared} keys compared"
                );
            }
        }
    }
}

#[test]
fn the_shared_sections_are_what_the_host_prints_for_layers_1_to_3() {
    for lang in ["en", "jp"] {
        for (name, ui_keys, pairs) in variants() {
            let cfg = config(lang, ui_keys, &pairs);
            let core = CoreKeys::from_keys_config(scheme_from_str(&cfg.ui.keys), &cfg.keys);
            let (s, _dir) = sim(cfg);
            for (ctx, cctx) in CONTEXTS {
                let facts = facts_for(&s.app, ctx);
                let shared = CoreLookup {
                    keys: &core,
                    ctx: cctx,
                };
                let secs: Vec<CoreSection> = core_sections(cctx, &facts, &shared, s.app.lang);
                let host = crate::ops::help::sections_for(&s.app, ctx);
                let mut rows = 0;
                for sec in &secs {
                    let h = host
                        .iter()
                        .find(|h| h.title == sec.title)
                        .unwrap_or_else(|| panic!("{lang} {name} {ctx:?}: no `{}`", sec.title));
                    assert!(
                        is_subsequence(&sec.rows, &h.rows),
                        "{lang} {name} {ctx:?} `{}`:\nshared {:#?}\nhost {:#?}",
                        sec.title,
                        sec.rows,
                        h.rows
                    );
                    rows += sec.rows.len();
                }
                // Not vacuous: a context always has something shared to show (at least `?`).
                assert!(rows >= 2, "{lang} {name} {ctx:?}: {rows} shared rows");
            }
        }
    }
}

#[test]
fn the_shared_footer_tokens_are_in_the_host_footer_in_order() {
    for lang in ["en", "jp"] {
        for (name, ui_keys, pairs) in variants() {
            let cfg = config(lang, ui_keys, &pairs);
            let core = CoreKeys::from_keys_config(scheme_from_str(&cfg.ui.keys), &cfg.keys);
            let (s, _dir) = sim(cfg);
            for (ctx, cctx) in CONTEXTS {
                // The shared entries of the host's own plan for this context, in its order.
                let plan: Vec<CoreFoot> = crate::ops::footer::plan(&s.app, ctx)
                    .into_iter()
                    .filter_map(|f| match f {
                        crate::ops::footer::Foot::Core(c) => Some(c),
                        _ => None,
                    })
                    .collect();
                assert!(plan.len() >= 4, "{lang} {name} {ctx:?}: plan {plan:?}");
                let facts = facts_for(&s.app, ctx);
                let shared = CoreLookup {
                    keys: &core,
                    ctx: cctx,
                };
                let toks: Vec<String> = footer_tokens(&plan, cctx, &facts, &shared, s.app.lang)
                    .into_iter()
                    .map(|t| t.text)
                    .collect();
                let host = crate::ops::footer::tokens(&s.app, ctx);
                let mut it = host.iter();
                for t in &toks {
                    assert!(
                        it.any(|h| h == t),
                        "{lang} {name} {ctx:?}: shared token `{t}` is not in the host footer (in order) {host:?}"
                    );
                }
                assert!(!toks.is_empty(), "{lang} {name} {ctx:?}: no shared token");
            }
        }
    }
}

/// The host's help overlay is the `Help` context: its title names the keys that close it.
fn host_title(cfg: Config) -> String {
    let (mut s, _dir) = sim(cfg);
    s.key('?');
    crate::ops::core_help::help_title(&s.app)
}

#[test]
fn the_host_help_title_names_the_keys_that_close_and_scroll() {
    assert_eq!(
        host_title(config("en", "vim", &[])),
        " Help  (?/Esc to close, j/k to scroll) "
    );
    // Rebinding the global help key renames it (`?` stays: a global key cannot lose its binding
    // to a different one, only the added key joins).
    let t = host_title(config("en", "vim", &[("global", "w", "toggle_help")]));
    assert!(t.contains("w") && t.contains("Esc to close"), "{t}");
    // Unbinding j from the help overlay drops it from the scroll keys.
    let t = host_title(config("en", "vim", &[("help", "j", "noop")]));
    assert!(t.contains("k to scroll") && !t.contains("j"), "{t}");
    // Both gone: no scroll clause at all.
    let t = host_title(config(
        "en",
        "vim",
        &[("help", "j", "noop"), ("help", "k", "noop")],
    ));
    assert_eq!(t, " Help  (?/Esc to close) ", "{t}");
    let t = host_title(config("jp", "vim", &[]));
    assert!(t.contains("で閉じる"), "{t}");
}

#[test]
fn q_with_the_help_open_quits_in_the_host_too() {
    // The app's `Q` quits even from the help overlay; so does the host (through its confirm
    // dialog by default).
    for confirm in [true, false] {
        let mut cfg = Config::default();
        cfg.ui.confirm_quit = confirm;
        let (mut s, _dir) = sim(cfg);
        s.key('?');
        assert!(s.app.show_help);
        s.key('Q');
        if confirm {
            assert!(!s.quit);
            assert_eq!(format!("{:?}", s.app.surface()), "DialogConfirmQuit");
        } else {
            assert!(s.quit, "Q must quit from the help");
        }
    }
}
