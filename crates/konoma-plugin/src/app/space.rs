//! The `Space` menu of the app: the operations that act on the screen, each under one key, as
//! konoma's menu of the preview lists them (`konoma_host_core::menu::build_items` orders and keys
//! them, [`crate::host::acts::menu_candidates`] and [`crate::host::acts::menu_label`] speak for
//! the plugin's declared actions).
//!
//! What is in it is what acts in the app: leaving, the `?` help and every declared action that
//! works now. The host's own operations (tabs, search, bookmarks, the editor, ...) are not here:
//! a menu names an operation only while its key acts, and those keys do nothing in the app.

use konoma_host_core::help::{default_label, menu_view, CoreLookup, HelpOp, KeyLookup};
use konoma_host_core::help_msg::CoreMsg;
use konoma_host_core::keymap::{CoreAction, CoreContext, CoreKeys, CoreMenu, KeyPress};
use konoma_host_core::lang::Lang;
use konoma_host_core::menu::{add_candidate, build_items, plain_key, Item};

use super::AppActs;
use crate::host::acts::{self as hacts, ActBinding, ActGate};

/// What an item of the menu runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SpaceOp {
    /// Leaves (or closes the help).
    Back,
    /// Opens the `?` help.
    Help,
    /// `Enter` over a table: the full-cell popup.
    Cell,
    /// A display option the `,` menu has too (the path style, the wrap).
    Core(CoreAction),
    /// The declared action at this place among the actions of the screen's key context.
    Declared(usize),
}

/// The menu on the screen of key context `act_ctx` (`ctx` is the same for the shared keys):
/// the items that act now, in the order and under the keys konoma gives them.
/// `lang_str` is the language the declared texts are looked up by. `enabled` says whether the session lets a declared action run now.
pub(super) fn items(
    keys: &CoreKeys,
    ctx: CoreContext,
    lang: Lang,
    lang_str: &str,
    acts: &AppActs,
    act_ctx: &str,
    enabled: impl Fn(&str) -> bool,
) -> Vec<Item<SpaceOp, String>> {
    let here = hacts::acts_in(&acts.bindings, act_ctx);
    // The declared actions by their place, the handle the menu core wants (`Copy`).
    let numbered: Vec<ActBinding<usize>> = here
        .iter()
        .enumerate()
        .map(|(i, b)| ActBinding {
            act: i,
            key: b.key,
            context: b.context.clone(),
            label: b.label.clone(),
            help: b.help.clone(),
            footer: b.footer,
            writes: b.writes,
        })
        .collect();
    let refs: Vec<&ActBinding<usize>> = numbered.iter().collect();
    let look = CoreLookup { keys, ctx };
    let own_key = |a: CoreAction| look.keys_of(HelpOp::Act(a)).into_iter().find_map(plain_key);

    let mut cands: Vec<(SpaceOp, Option<KeyPress>)> = Vec::new();
    add_candidate(&mut cands, SpaceOp::Back, own_key(CoreAction::PreviewBack));
    add_candidate(&mut cands, SpaceOp::Help, own_key(CoreAction::ToggleHelp));
    if ctx == CoreContext::Table {
        add_candidate(&mut cands, SpaceOp::Cell, None);
    }
    // The display options the screen's `,` menu offers, in the order konoma lists them (the
    // path style, then the wrap); they have no key of their own.
    let options = menu_view(keys, ctx, CoreMenu::Options, lang, false);
    let option_label = |a: CoreAction| {
        options
            .as_ref()
            .and_then(|v| v.items.iter().find(|i| i.action == a))
            .map(|i| i.label.clone())
    };
    for a in [CoreAction::CyclePathStyle, CoreAction::ToggleWrap] {
        if option_label(a).is_some() {
            add_candidate(&mut cands, SpaceOp::Core(a), None);
        }
    }
    // The declared actions come last, as konoma lists them: after what the screen has by itself.
    for (i, key) in hacts::menu_candidates(&refs) {
        add_candidate(&mut cands, SpaceOp::Declared(i), key);
    }

    // The footer plan decides the order: the declared actions that asked for a footer entry come
    // right after the screen's first entry, which is the way back on a canvas only (the other
    // screens' first entries are their scrolling keys); the help closes the plan.
    let declared: Vec<SpaceOp> = hacts::footer_order(&refs)
        .into_iter()
        .map(|b| SpaceOp::Declared(b.act))
        .collect();
    let mut prio: Vec<SpaceOp> = Vec::new();
    if ctx == CoreContext::Canvas {
        prio.push(SpaceOp::Back);
        prio.extend(declared);
    } else {
        prio.extend(declared);
        prio.push(SpaceOp::Cell);
        prio.push(SpaceOp::Back);
    }
    prio.push(SpaceOp::Help);

    build_items(
        cands,
        &prio,
        &[],
        &[],
        |_| None,
        |op| match op {
            SpaceOp::Back | SpaceOp::Help | SpaceOp::Cell | SpaceOp::Core(_) => true,
            SpaceOp::Declared(i) => {
                let b = here[i];
                hacts::gate(Some(b), enabled(&b.act), acts.read_only) == ActGate::On
            }
        },
        |op| match op {
            SpaceOp::Back => {
                default_label(ctx, CoreAction::PreviewBack).map(|m| m.tr(lang).to_string())
            }
            SpaceOp::Help => {
                default_label(ctx, CoreAction::ToggleHelp).map(|m| m.tr(lang).to_string())
            }
            SpaceOp::Cell => Some(CoreMsg::HintViewCell.tr(lang).to_string()),
            SpaceOp::Core(a) => option_label(a),
            SpaceOp::Declared(i) => hacts::menu_label(&refs, &i, lang_str).map(str::to_string),
        },
    )
}

/// The declared action `op` stands for, among the actions of `act_ctx`.
pub(super) fn declared<'a>(
    acts: &'a AppActs,
    act_ctx: &str,
    op: SpaceOp,
) -> Option<&'a ActBinding<String>> {
    match op {
        SpaceOp::Declared(i) => hacts::acts_in(&acts.bindings, act_ctx).get(i).copied(),
        _ => None,
    }
}
