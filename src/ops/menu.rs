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

use crate::app::App;
use crate::i18n::{tr, Msg};
use crate::keymap::{Action, Context, KeyPress, LeaderId};

use super::{action_enabled, decl, keys};

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
    /// The `?` help: the fixed menus show every item (the code-block copy included).
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
fn fixed_menu(app: &App, ctx: Context, id: LeaderId, view: View) -> Option<Menu> {
    let m = app.keymaps.menu(ctx, id)?;
    // `,` offers what acts here; the copy and file menus have always listed all their items
    // (the code-block copy only while a code block is focused).
    let gated = |a: Action| match id {
        LeaderId::Options => action_enabled(app, ctx, a),
        _ => {
            view == View::Help || !matches!(a, Action::CopyCodeBlock) || action_enabled(app, ctx, a)
        }
    };
    let items = m
        .items
        .iter()
        .filter(|it| gated(it.action))
        .map(|it| {
            let label = match it.label {
                // A user-added entry has no label of its own: name it as the declaration does.
                Msg::Empty => label_of(app, ctx, it.action),
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
        Context::Image | Context::Table => v.push(Action::CyclePathStyle),
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

/// What `a` is called in a menu of `ctx`: the declaration's label as it reads now, else its
/// description, else a label of its own. `None` = the operation is not named anywhere, so it is
/// not offered.
fn label_of(app: &App, ctx: Context, a: Action) -> Option<Msg> {
    let t = decl::text(app, ctx, a);
    t.label.or(t.help).or_else(|| special_label(a))
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

/// The letters tried for an operation that has neither a free key of its own nor a mnemonic.
/// `q` is not among them: inside a menu it closes it.
const FREE_LETTERS: &str = "abcdefghijklmnoprstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";

/// Every operation `ctx` can run, in the order the menu lists them: the keyed ones by their key
/// (with that key when it is a plain character), then the fixed keys' operations, then the ones
/// without a key. Movement and the closing of overlays are not operations to offer.
fn candidates(app: &App, ctx: Context) -> Vec<(Action, Option<KeyPress>)> {
    let mut out: Vec<(Action, Option<KeyPress>)> = Vec::new();
    let mut add = |a: Action, key: Option<KeyPress>| {
        if matches!(
            a,
            Action::Noop | Action::Navigate(_) | Action::TabGoto(_) | Action::Cancel
        ) {
            return;
        }
        if label_of(app, ctx, a).is_none() {
            return;
        }
        match out.iter_mut().find(|(o, _)| *o == a) {
            // The first plain key of an operation is the one the menu uses.
            Some((_, k)) => {
                if k.is_none() {
                    *k = key;
                }
            }
            None => out.push((a, key)),
        }
    };
    let plain = |k: KeyPress| -> Option<KeyPress> {
        (!k.ctrl && matches!(k.code, crossterm::event::KeyCode::Char(c) if c != 'q' && c != ' '))
            .then_some(k)
    };
    for (k, a) in app.keymaps.bound_actions(ctx) {
        add(a, plain(k));
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
    out
}

/// The key each candidate gets. Keys the operations already have come first (so a habit carries
/// over), then a mnemonic letter, then the first free letter.
fn assign(cands: &[(Action, Option<KeyPress>)]) -> Vec<Option<KeyPress>> {
    let mut taken: Vec<KeyPress> = Vec::new();
    let mut chosen: Vec<Option<KeyPress>> = vec![None; cands.len()];
    for (i, (_, k)) in cands.iter().enumerate() {
        if let Some(k) = k {
            if !taken.contains(k) {
                taken.push(*k);
                chosen[i] = Some(*k);
            }
        }
    }
    for (i, (a, _)) in cands.iter().enumerate() {
        if chosen[i].is_some() {
            continue;
        }
        let pick = mnemonic(*a)
            .map(KeyPress::ch)
            .filter(|k| !taken.contains(k))
            .or_else(|| {
                FREE_LETTERS
                    .chars()
                    .map(KeyPress::ch)
                    .find(|k| !taken.contains(k))
            });
        if let Some(k) = pick {
            taken.push(k);
            chosen[i] = Some(k);
        }
    }
    // More operations than letters cannot happen with today's contexts; an operation left
    // without a letter is simply not in the menu (it keeps its own key).
    chosen
}

fn actions_menu(app: &App, ctx: Context) -> Menu {
    let cands = candidates(app, ctx);
    let letters = assign(&cands);
    let items = cands
        .iter()
        .zip(letters)
        .filter(|((a, _), _)| action_enabled(app, ctx, *a))
        .filter_map(|((a, _), key)| {
            let key = key?;
            let label = label_of(app, ctx, *a)?;
            Some(MenuItem {
                key,
                action: *a,
                label: tr(app.lang, label).to_string(),
            })
        })
        .collect();
    Menu {
        id: LeaderId::Actions,
        title: tr(app.lang, Msg::WkActionsTitle).to_string(),
        items,
    }
}
