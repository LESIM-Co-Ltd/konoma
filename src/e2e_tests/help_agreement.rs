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

/// A key set: (name, `ui.keys`, `[keys.text]` pairs).
type Variant = (
    &'static str,
    &'static str,
    Vec<(&'static str, &'static str)>,
);

/// The key sets to try: (name, `ui.keys`, `[keys.text]` pairs).
fn variants() -> Vec<Variant> {
    vec![
        ("default", "vim", vec![]),
        (
            "rebind",
            "vim",
            vec![("x", "toggle_help"), ("H", "navigate_down")],
        ),
        ("unbind q", "vim", vec![("q", "noop")]),
        ("unbind q and Q", "vim", vec![("q", "noop"), ("Q", "noop")]),
        ("less", "less", vec![]),
        ("less and unbind", "less", vec![("j", "noop")]),
    ]
}

fn config(lang: &str, ui_keys: &str, pairs: &[(&str, &str)]) -> Config {
    let mut cfg = Config::default();
    cfg.ui.lang = lang.into();
    cfg.ui.keys = ui_keys.into();
    let mut t = std::collections::HashMap::new();
    for (k, a) in pairs {
        t.insert((*k).to_string(), (*a).to_string());
    }
    if !t.is_empty() {
        cfg.keys.surfaces.insert("text".to_string(), t);
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
    for (lang, _) in [("en", ()), ("jp", ())] {
        for (name, ui_keys, pairs) in variants() {
            let cfg = config(lang, ui_keys, &pairs);
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
                    assert_eq!(h, c, "{lang} {name} {ctx:?}: keys of {op:?}");
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
                            "{lang} {name} {ctx:?}: layer of {kp:?}"
                        );
                        compared += 1;
                    }
                }
                assert!(
                    compared >= 3,
                    "{lang} {name} {ctx:?}: only {compared} keys compared"
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
    // The host orders its footer by priority; this is the order it puts these in.
    let plan = [
        CoreFoot::Scroll,
        CoreFoot::Back,
        CoreFoot::Help,
        CoreFoot::Ends,
        CoreFoot::PageKeys,
    ];
    for lang in ["en", "jp"] {
        for (name, ui_keys, pairs) in variants() {
            let cfg = config(lang, ui_keys, &pairs);
            let core = CoreKeys::from_keys_config(scheme_from_str(&cfg.ui.keys), &cfg.keys);
            let (s, _dir) = sim(cfg);
            let ctx = Context::Text;
            let facts = facts_for(&s.app, ctx);
            let shared = CoreLookup {
                keys: &core,
                ctx: CoreContext::Text,
            };
            let toks: Vec<String> =
                footer_tokens(&plan, CoreContext::Text, &facts, &shared, s.app.lang)
                    .into_iter()
                    .map(|t| t.text)
                    .collect();
            let host = crate::ops::footer::tokens(&s.app, ctx);
            let mut it = host.iter();
            for t in &toks {
                assert!(
                    it.any(|h| h == t),
                    "{lang} {name}: shared token `{t}` is not in the host footer (in order) {host:?}"
                );
            }
            assert!(!toks.is_empty(), "{lang} {name}: no shared token");
        }
    }
}
