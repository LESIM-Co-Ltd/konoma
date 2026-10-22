//! Tests of the `Space` menu's own rules (they came out of a mutation audit): which operations are
//! candidates, which letter each gets, in what order, and when a menu opens at all. They use the
//! private pieces of `menu` directly where a state of the app cannot reach a case.

use std::collections::HashMap;

use super::*;
use crate::config::Config;
use crate::keymap::{Action as A, Context, KeyPress, LeaderId};
use crate::test_support::{unique_tmp, TmpDir};

fn app_with(cfg: Config) -> (App, TmpDir) {
    let dir = unique_tmp("konoma_ops_menu_tests");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.txt"), b"hello\n").unwrap();
    (App::new(dir.to_path_buf(), cfg).unwrap(), dir)
}

fn cfg_keys(table: &str, pairs: &[(&str, &str)]) -> Config {
    let mut cfg = Config::default();
    let mut t = HashMap::new();
    for (k, a) in pairs {
        t.insert((*k).to_string(), (*a).to_string());
    }
    cfg.keys.surfaces.insert(table.to_string(), t);
    cfg
}

fn declared(app: &App, ctx: Context) -> Vec<MenuItem> {
    declared_actions_menu(app, ctx)
}

// ---- the letters ------------------------------------------------------------------------------

/// An operation's letter is the key it already has when that is free; two operations that arrive
/// with the same key do not both get it.
#[test]
fn a_letter_is_given_to_one_operation_only() {
    let x = KeyPress::ch('x');
    let cands = [(A::ToggleWrap, Some(x)), (A::CyclePathStyle, Some(x))];
    let letters = assign(&cands);
    assert_eq!(letters[0], Some(x), "the first keeps its key");
    assert!(letters[1].is_some(), "the second still has a place");
    assert_ne!(letters[1], Some(x), "but not the same letter");
    // The free letters run `a`, `b`, ... for the ones with nothing of their own.
    let cands = [
        (A::Refresh, None),
        (A::SearchNext, None),
        (A::SearchPrev, None),
    ];
    let letters = assign(&cands);
    assert_eq!(
        letters,
        [
            Some(KeyPress::ch('a')),
            Some(KeyPress::ch('b')),
            Some(KeyPress::ch('c'))
        ]
    );
    // Whatever the mix, no two share a letter.
    let cands = [
        (A::Refresh, Some(KeyPress::ch('a'))),
        (A::SearchNext, None),
        (A::SearchPrev, None),
        (A::ToggleWrap, None),
    ];
    let letters: Vec<_> = assign(&cands).into_iter().flatten().collect();
    for (i, a) in letters.iter().enumerate() {
        assert!(letters[i + 1..].iter().all(|b| a != b), "{letters:?}");
    }
}

/// Each mnemonic is the letter that says what the operation is.
#[test]
fn mnemonics_are_pinned() {
    let alone = |a: Action| assign(&[(a, None)])[0];
    assert_eq!(alone(A::BookmarkDelete), Some(KeyPress::ch('d')));
    assert_eq!(alone(A::BookmarkEdit), Some(KeyPress::ch('e')));
    assert_eq!(alone(A::CyclePathStyle), Some(KeyPress::ch('p')));
    assert_eq!(alone(A::ToggleWrap), Some(KeyPress::ch('w')));
    assert_eq!(alone(A::ToggleFocusedItem), Some(KeyPress::ch('t')));
    #[cfg(feature = "git")]
    {
        assert_eq!(alone(A::ToggleFollowDiffScope), Some(KeyPress::ch('f')));
        assert_eq!(alone(A::GitOpenLog), Some(KeyPress::ch('l')));
        assert_eq!(alone(A::GitOpenGraph), Some(KeyPress::ch('g')));
    }
    // An operation with none takes the first free letter.
    assert_eq!(alone(A::Refresh), Some(KeyPress::ch('a')));
}

/// A mnemonic that another operation already holds gives way to the next free letter.
#[test]
fn a_taken_mnemonic_gives_way_to_a_free_letter() {
    let p = KeyPress::ch('p');
    let letters = assign(&[(A::ToggleWrap, Some(p)), (A::CyclePathStyle, None)]);
    assert_eq!(letters[0], Some(p));
    assert_eq!(letters[1], Some(KeyPress::ch('a')), "`p` is taken");
    // And the free letters skip the ones taken, in order.
    let letters = assign(&[
        (A::Refresh, Some(KeyPress::ch('a'))),
        (A::SearchNext, Some(KeyPress::ch('b'))),
        (A::SearchPrev, None),
    ]);
    assert_eq!(letters[2], Some(KeyPress::ch('c')));
}

/// `q` closes a menu and `Space` is not a letter to choose: neither is ever an item's key, even
/// when an operation is bound to it.
#[test]
fn q_and_space_are_never_the_key_of_an_item() {
    for key in ["space", "q"] {
        let (app, _d) = app_with(cfg_keys("text", &[(key, "toggle_wrap")]));
        let items = declared(&app, Context::Text);
        let wrap = items
            .iter()
            .find(|i| i.action == A::ToggleWrap)
            .unwrap_or_else(|| panic!("{key}: wrap is in the menu"));
        assert_eq!(wrap.key, KeyPress::ch('w'), "{key}: it takes its mnemonic");
        for i in &items {
            assert!(
                i.key != KeyPress::ch('q') && i.key != KeyPress::ch(' '),
                "{key}: `{}` is an item's key",
                i.key.label()
            );
        }
    }
    // Whatever the context, the letters are unique and never `q`.
    let (app, _d) = app_with(Config::default());
    for ctx in Context::all() {
        let items = declared(&app, ctx);
        for (i, a) in items.iter().enumerate() {
            assert!(a.key != KeyPress::ch('q') && a.key != KeyPress::ch(' '));
            assert!(
                items[i + 1..].iter().all(|b| b.key != a.key),
                "{ctx:?}: `{}` twice",
                a.key.label()
            );
        }
    }
}

// ---- the order --------------------------------------------------------------------------------

/// Footer plan first, then the help's own rows, then the ones neither names, then the rows every
/// screen lists.
#[test]
fn rank_orders_the_footer_plan_then_own_rows_then_the_rest_then_everywhere() {
    let prio = vec![
        (A::Refresh, None),
        (A::SearchNext, None),
        (A::TabNext, None),
    ];
    let own = [A::Refresh, A::ToggleWrap];
    let everywhere = [A::TabNext, A::ToggleHelp];
    assert_eq!(rank(A::Refresh, &prio, &own, &everywhere), (0, 0, 0));
    assert_eq!(
        rank(A::ToggleWrap, &prio, &own, &everywhere),
        (usize::MAX, 0, 1)
    );
    assert_eq!(rank(A::TabNext, &prio, &own, &everywhere), (2, 2, 0));
    assert_eq!(
        rank(A::ToggleHelp, &prio, &own, &everywhere),
        (usize::MAX, 2, 1)
    );
    assert_eq!(rank(A::SearchNext, &prio, &own, &everywhere), (1, 1, 0));
    assert_eq!(
        rank(A::SearchPrev, &prio, &own, &everywhere),
        (usize::MAX, 1, 0)
    );
    // So an "everywhere" row sorts after one of the screen's own rows with the same plan place,
    // and a leftover sits between them.
    let mut v = [
        (rank(A::ToggleHelp, &[], &own, &everywhere), "everywhere"),
        (rank(A::SearchPrev, &[], &own, &everywhere), "leftover"),
        (rank(A::ToggleWrap, &[], &own, &everywhere), "own"),
    ];
    v.sort();
    assert_eq!(v.map(|x| x.1), ["own", "leftover", "everywhere"]);
}

// ---- what is offered --------------------------------------------------------------------------

/// Movement, the closing of overlays and no-ops are not operations to offer, in any context and
/// either language, and not when a key is bound to one on purpose.
#[test]
fn no_menu_offers_a_cancel_a_no_op_or_a_movement() {
    let mut cfg = cfg_keys(
        "text",
        &[("x", "noop"), ("z", "cancel"), ("w", "navigate:down")],
    );
    cfg.keys.surfaces.insert(
        "document".into(),
        HashMap::from([("x".to_string(), "cancel".to_string())]),
    );
    let (mut app, _d) = app_with(cfg);
    let mut looked = 0;
    for lang in [crate::i18n::Lang::En, crate::i18n::Lang::Jp] {
        app.lang = lang;
        for ctx in Context::all() {
            for i in declared(&app, ctx) {
                looked += 1;
                assert!(
                    !matches!(
                        i.action,
                        A::Noop | A::Navigate(_) | A::TabGoto(_) | A::Cancel
                    ),
                    "{lang:?} {ctx:?}: {:?} is offered as `{}`",
                    i.action,
                    i.label
                );
            }
        }
    }
    assert!(looked > 100, "looked at {looked} items");
}

/// The operations a context has with no key by default are all in its menu: it is where they are
/// always reachable.
#[test]
fn every_unkeyed_operation_is_in_the_menu_with_its_letter() {
    let (app, _d) = app_with(Config::default());
    let mut checked = 0;
    for ctx in Context::all() {
        let items = declared(&app, ctx);
        for a in unkeyed(ctx) {
            checked += 1;
            assert!(
                items.iter().any(|i| i.action == a),
                "{ctx:?}: {a:?} is not in the menu"
            );
        }
    }
    let least = if cfg!(feature = "git") { 10 } else { 6 };
    assert!(checked >= least, "checked {checked}");
}

/// A key that is pressed in a menu resolves to an item that acts now, and to nothing else.
#[test]
fn a_menu_key_resolves_only_the_items_the_menu_holds() {
    let (app, _d) = app_with(Config::default());
    for ctx in Context::all() {
        let Some(m) = menu(&app, ctx, LeaderId::Actions, View::Open) else {
            continue;
        };
        for it in &m.items {
            assert_eq!(find(&app, ctx, LeaderId::Actions, it.key), Some(it.action));
        }
        // A letter nothing has.
        assert_eq!(
            find(&app, ctx, LeaderId::Actions, KeyPress::ch('\u{3042}')),
            None
        );
        assert_eq!(find(&app, ctx, LeaderId::Actions, KeyPress::ch('q')), None);
    }
}

/// A menu with nothing to offer does not open: `opens` is `false` for a menu that exists but is
/// empty (every item taken away by `[keys]`), and `true` exactly when it has an item.
#[test]
fn a_menu_with_no_item_does_not_open() {
    let (app, _d) = app_with(Config::default());
    assert!(opens(&app, Context::Image, LeaderId::Options));
    // The menu is there, with nothing in it.
    let (app, _d) = app_with(cfg_keys("image", &[(", p", "noop")]));
    let m = menu(&app, Context::Image, LeaderId::Options, View::Open).unwrap();
    assert!(m.items.is_empty());
    assert!(!opens(&app, Context::Image, LeaderId::Options));
    // A context with no such menu at all does not open either.
    assert!(menu(&app, Context::Info, LeaderId::Options, View::Open).is_none());
    assert!(!opens(&app, Context::Info, LeaderId::Options));
    // And `opens` agrees with the menu everywhere.
    let (app, _d) = app_with(Config::default());
    let mut full = 0;
    for ctx in Context::all() {
        for id in [
            LeaderId::Actions,
            LeaderId::Options,
            LeaderId::Copy,
            LeaderId::File,
        ] {
            let m = menu(&app, ctx, id, View::Open);
            let want = m.as_ref().is_some_and(|m| !m.items.is_empty());
            assert_eq!(opens(&app, ctx, id), want, "{ctx:?} {id:?}");
            full += usize::from(want);
        }
    }
    assert!(full > 20, "{full} menus with items");
}

// ---- the key tables the menus and the help read -----------------------------------------------

/// A group of exactly two operations is zipped into columns (`J K`), anything else is one key per
/// column.
#[test]
fn a_pair_of_operations_is_zipped_and_any_other_group_is_one_key_per_column() {
    let (app, _d) = app_with(Config::default());
    let (cols, zipped) = keys::columns(&app, Context::Text, &[A::SearchNext, A::SearchPrev]);
    assert!(zipped);
    assert_eq!(cols.len(), 1);
    assert_eq!(cols[0].len(), 2, "n and N share a column");
    let (cols, zipped) = keys::columns(&app, Context::Text, &[A::SearchNext]);
    assert!(!zipped);
    assert_eq!(cols.len(), 1);
    assert_eq!(cols[0].len(), 1);
    let (cols, zipped) = keys::columns(
        &app,
        Context::Text,
        &[A::SearchNext, A::SearchPrev, A::SearchStart],
    );
    assert!(!zipped);
    assert_eq!(cols.len(), 3);
    assert!(cols.iter().all(|c| c.len() == 1));
}

/// The screens that write `Enter` as `↵` in the footer are the previews of text, documents,
/// pictures, tables and a plugin's canvas, and no others.
#[test]
fn only_the_preview_screens_write_enter_short() {
    for (ctx, want) in [
        (Context::Text, true),
        (Context::Document, true),
        (Context::Image, true),
        (Context::Table, true),
        (Context::Canvas, true),
        (Context::Tree, false),
        (Context::Bookmarks, false),
        (Context::Tabs, false),
        (Context::Outline, false),
        (Context::Info, false),
        (Context::Help, false),
        (Context::TextVisual, false),
        (Context::TableCell, false),
    ] {
        assert_eq!(keys::short_enter(ctx), want, "{ctx:?}");
    }
    #[cfg(feature = "git")]
    assert!(!keys::short_enter(Context::Diff));
}
