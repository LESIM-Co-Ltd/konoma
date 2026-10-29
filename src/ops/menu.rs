// The menus behind `y`, `Space` and `,`: what a menu holds in a context, as the which-key line
// draws it, the key handler resolves it and the `?` help lists it (plugin stage 2 / B3b,
// `docs/FEATURE-PLUGIN-ARCHITECTURE.md` §10-3-3 layer 1).
//
// One mechanism for all of them. A menu is *what the context lets you do with that key*:
//
// - `y` copies: the copy menu of the context (path, table cell, commit info, branch name), kept in
//   the keymap (`KeyMap::menu`) so `[keys.<context>] "y x"` can change one context's menu.
// - `,` sorts and shows: the options of the context (the tree sorts, a preview wraps, a diff picks
//   what the follow shows), also kept in the keymap.
// - `Space` operates. In the tree it is the file menu (kept in the keymap, as before); everywhere
//   else it is *built here* from the operation declarations: every operation that acts in the
//   context now (`ops::gate`), each under the single key it already has when that is free, else a
//   letter of its own. So an operation that lost its key — to a conflict, or to a rebinding — is
//   still one `Space` away (design principle 5).
//
// The items of the two built menus are decided over *all* the candidate operations, before the
// gate is asked: a letter does not move because another operation happens not to act right now.
//
// The `Space` menu lists its items by how often the context uses them — the order of the footer
// plan (`footer::priority`: the footer cuts from the end, so the front is what matters most), then
// the rest in the order the help declares them, then whatever neither names — and calls each by
// the footer's short label, not by the help's sentence.

use crate::app::App;
use crate::i18n::{tr, Msg};
use crate::keymap::{Action, Context, KeyPress, LeaderId};

use super::{action_enabled, decl, footer, help, keys};
use crate::plugins::PluginAct;
use konoma_host_core::menu::{self as menu_core, add_candidate, plain_key};

/// One entry of an open menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuItem {
    pub key: KeyPress,
    pub action: Action,
    /// The label in the language of the interface.
    pub label: String,
}

/// A menu as it stands now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Menu {
    pub id: LeaderId,
    pub title: String,
    pub items: Vec<MenuItem>,
}

/// What `menu` is asked for: the menu as it opens (only what acts now) or as the help lists it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    /// The which-key line and the key handler: an item that does not act is not offered.
    Open,
    /// The `?` help: the same items as `Open` today (a hint is shown only while its key acts, so
    /// the code-block copy is listed only while a code block is focused). Kept as its own entry so
    /// a caller says which of the two it is.
    Help,
}

/// The menu `id` of `ctx`, or `None` when `ctx` has no such menu.
pub fn menu(app: &App, ctx: Context, id: LeaderId, view: View) -> Option<Menu> {
    match id {
        LeaderId::Actions => Some(actions_menu(app, ctx)),
        _ => fixed_menu(app, ctx, id, view),
    }
}

/// The operation key `kp` runs in the open menu `id` of `ctx` (`None` = the key is not in it).
pub fn find(app: &App, ctx: Context, id: LeaderId, kp: KeyPress) -> Option<Action> {
    menu(app, ctx, id, View::Open)?
        .items
        .into_iter()
        .find(|it| it.key == kp)
        .map(|it| it.action)
}

/// Whether pressing the key that opens `id` in `ctx` opens anything: a menu with nothing to offer
/// does not open (the key then does nothing).
pub fn opens(app: &App, ctx: Context, id: LeaderId) -> bool {
    menu(app, ctx, id, View::Open).is_some_and(|m| !m.items.is_empty())
}

/// A menu kept in the keymap (`y`, `,`, the tree's `Space`).
fn fixed_menu(app: &App, ctx: Context, id: LeaderId, _view: View) -> Option<Menu> {
    let m = app.keymaps.menu(ctx, id)?;
    // `,` offers what acts here; the copy and file menus list all their items (the code-block
    // copy only while a code block is focused: the menu, the footer and the help alike).
    let gated = |a: Action| match id {
        LeaderId::Options => action_enabled(app, ctx, a),
        _ => !matches!(a, Action::CopyCodeBlock) || action_enabled(app, ctx, a),
    };
    let items = m
        .items
        .iter()
        .filter(|it| gated(it.action))
        .map(|it| {
            let label = match it.label {
                // A user-added entry has no label of its own: name it as the declaration does.
                Msg::Empty => label_of(app, ctx, it.action, &footer::priority(app, ctx)),
                m => Some(crate::ui::status::relabel_for_backend(app, m)),
            };
            MenuItem {
                key: it.key,
                action: it.action,
                label: label
                    .map(|l| tr(app.lang, l).to_string())
                    .unwrap_or_default(),
            }
        })
        .collect();
    Some(Menu {
        id,
        title: tr(app.lang, m.title).to_string(),
        items,
    })
}

// ---------------------------------------------------------------------------------------------
// The `Space` menu outside the tree.
// ---------------------------------------------------------------------------------------------

/// Operations a context has that no key is bound to by default (they used to have one, or never
/// did): the menu is where they are always reachable.
fn unkeyed(ctx: Context) -> Vec<Action> {
    let mut v: Vec<Action> = Vec::new();
    match ctx {
        Context::Text => v.extend([Action::CyclePathStyle, Action::ToggleWrap]),
        Context::Document => v.extend([
            Action::CyclePathStyle,
            Action::ToggleWrap,
            Action::ToggleFocusedItem,
        ]),
        Context::Image | Context::Table | Context::Canvas => v.push(Action::CyclePathStyle),
        Context::Bookmarks => v.push(Action::BookmarkDelete),
        #[cfg(feature = "git")]
        Context::Diff => v.extend([Action::ToggleFollowDiffScope, Action::CyclePathStyle]),
        #[cfg(feature = "git")]
        Context::Screen(_, crate::keymap::Screen::Changes) => {
            v.extend([Action::GitOpenLog, Action::GitOpenGraph])
        }
        _ => {}
    }
    v
}

/// A label for an operation the declarations leave unnamed because it has no footer or help row
/// of its own (the bookmark and tab lists' operations).
fn special_label(a: Action) -> Option<Msg> {
    Some(match a {
        Action::BookmarkEdit => Msg::HintEdit,
        Action::BookmarkDelete => Msg::WkDelete,
        Action::BookmarkJump | Action::TabListActivate => Msg::HintOpen,
        Action::BookmarkClose | Action::InfoClose => Msg::FtClose,
        Action::TabListClose => Msg::HintCloseTab,
        Action::OutlineJump => Msg::HintJump,
        _ => return None,
    })
}

/// A short label for an operation that has none in the footer or its declaration (the help's
/// sentence is too long for a menu line). `search` in a footer is `n/N:match`, a status; here the
/// two directions are items of their own.
fn short_label(a: Action) -> Option<Msg> {
    Some(match a {
        Action::SearchNext => Msg::WkNextMatch,
        Action::SearchPrev => Msg::WkPrevMatch,
        Action::Refresh => Msg::WkRefresh,
        Action::TabNewAfter => Msg::HintNewTab,
        _ => return None,
    })
}

/// What `a` is called in a menu of `ctx`: the menu's own label where the footer's would not tell
/// it apart from its pair (`decl::menu_label`), else the footer's label for it (`prio` is the footer plan,
/// `footer::priority`), else the declaration's label as it reads now, else a short label of the
/// menu's own. Never the help's sentence. `None` = the operation is not named, so it is not
/// offered.
fn label_of(app: &App, ctx: Context, a: Action, prio: &[(Action, Option<Msg>)]) -> Option<Msg> {
    // The few operations whose footer label is shared with their pair say which way in a menu.
    decl::menu_label(app, ctx, a)
        .or_else(|| {
            prio.iter()
                .find_map(|(p, l)| (*p == a).then_some(*l).flatten())
        })
        .or_else(|| decl::text(app, ctx, a).label)
        .or_else(|| short_label(a))
        .or_else(|| special_label(a))
}

/// A letter that says what the operation is, tried before the free letters in order.
fn mnemonic(a: Action) -> Option<char> {
    Some(match a {
        Action::BookmarkDelete => 'd',
        Action::BookmarkEdit => 'e',
        Action::CyclePathStyle => 'p',
        Action::ToggleWrap => 'w',
        Action::ToggleFocusedItem => 't',
        #[cfg(feature = "git")]
        Action::ToggleFollowDiffScope => 'f',
        #[cfg(feature = "git")]
        Action::GitOpenLog => 'l',
        #[cfg(feature = "git")]
        Action::GitOpenGraph => 'g',
        _ => return None,
    })
}

/// Every operation `ctx` can run, in the order the menu lists them: the keyed ones by their key
/// (with that key when it is a plain character), then the fixed keys' operations, then the ones
/// without a key. Movement and the closing of overlays are not operations to offer.
fn candidates(
    app: &App,
    ctx: Context,
    prio: &[(Action, Option<Msg>)],
) -> Vec<(Action, Option<KeyPress>)> {
    let mut out: Vec<(Action, Option<KeyPress>)> = Vec::new();
    let mut add = |a: Action, key: Option<KeyPress>| {
        if matches!(
            a,
            Action::Noop | Action::Navigate(_) | Action::TabGoto(_) | Action::Cancel
        ) {
            return;
        }
        // A plugin's operation is named by the plugin, not by a message of ours.
        if !matches!(a, Action::Plugin(_))
            && decl::repr_label(app, a).is_none()
            && label_of(app, ctx, a, prio).is_none()
        {
            return;
        }
        // The first plain key of an operation is the one the menu uses.
        add_candidate(&mut out, a, key);
    };
    for (k, a) in app.keymaps.bound_actions(ctx) {
        add(a, plain_key(k));
    }
    // What the context has by default but no key reaches now (moved away by `[keys]`, or lost
    // to a conflict).
    for &a in app.keymaps.default_ops.get(&ctx).into_iter().flatten() {
        add(a, None);
    }
    for f in keys::fixed_bindings(ctx) {
        if !f.alias {
            add(f.action, None);
        }
    }
    for a in unkeyed(ctx) {
        add(a, None);
    }
    // Every operation the open plugin declared, keyed or not: the menu is how an action that lost
    // its key (a collision, a rebinding) is still reachable.
    for (act, key) in konoma_plugin::host::acts::menu_candidates(&app.plugin_acts_in(ctx)) {
        add(Action::Plugin(act), key);
    }
    out
}

/// The shared assignment, with this host's mnemonics (the menu tests call it directly).
#[cfg(test)]
fn assign(cands: &[(Action, Option<KeyPress>)]) -> Vec<Option<KeyPress>> {
    menu_core::assign(cands, mnemonic)
}

/// The shared rank, over the footer plan's pairs (the menu tests call it directly).
#[cfg(test)]
fn rank(
    a: Action,
    prio: &[(Action, Option<Msg>)],
    own: &[Action],
    everywhere: &[Action],
) -> (usize, u8, usize) {
    let prio: Vec<Action> = prio.iter().map(|(p, _)| *p).collect();
    menu_core::rank(a, &prio, own, everywhere)
}

/// The label of a plugin's operation in the language of the interface.
fn plugin_label(app: &App, ctx: Context, p: PluginAct) -> Option<String> {
    konoma_plugin::host::acts::menu_label(&app.plugin_acts_in(ctx), &p, app.plugin_lang())
        .map(str::to_string)
}

/// The `Space` menu of `ctx` as it is built: every item (when `gated` is false) or the ones that
/// act now.
fn build_actions(app: &App, ctx: Context, gated: bool) -> Vec<MenuItem> {
    let prio = footer::priority(app, ctx);
    let cands = candidates(app, ctx, &prio);
    let (own, everywhere) = help::declared_groups(app, ctx);
    let prio_actions: Vec<Action> = prio.iter().map(|(a, _)| *a).collect();
    menu_core::build_items(
        cands,
        &prio_actions,
        &own,
        &everywhere,
        mnemonic,
        |a| !gated || action_enabled(app, ctx, a),
        |a| match a {
            Action::Plugin(p) => plugin_label(app, ctx, p),
            _ if decl::repr_label(app, a).is_some() => decl::repr_label(app, a),
            _ => label_of(app, ctx, a, &prio).map(|m| tr(app.lang, m).to_string()),
        },
    )
    .into_iter()
    .map(|i| MenuItem {
        key: i.key,
        action: i.action,
        label: i.label,
    })
    .collect()
}

/// Every item the `Space` menu of `ctx` can hold, whether or not it acts right now, in menu order.
#[cfg(test)]
pub fn declared_actions_menu(app: &App, ctx: Context) -> Vec<MenuItem> {
    build_actions(app, ctx, false)
}

fn actions_menu(app: &App, ctx: Context) -> Menu {
    let items = build_actions(app, ctx, true);
    Menu {
        id: LeaderId::Actions,
        title: tr(app.lang, Msg::WkActionsTitle).to_string(),
        items,
    }
}

#[cfg(test)]
#[path = "menu_tests.rs"]
mod tests;
