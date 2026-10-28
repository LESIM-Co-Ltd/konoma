// Which keys run an operation in a context, and how a key is written for a person.
//
// The keymap (`crate::keymap`) holds the rebindable keys. The fixed keys (`Enter`, `Tab`,
// `Shift-Tab`, `Esc`, `Space` in a document, the arrows) cannot be rebound, but each has a default
// *operation* per context, declared here. `main::handle_fixed_key` looks the operation up here and
// runs it through `dispatch_action`, and the help and the footer name the key from the same table.

use crossterm::event::KeyCode;

use crate::app::App;
#[cfg(feature = "git")]
use crate::keymap::Screen;
use crate::keymap::{Action, Context, KeyPress, Motion};

/// One fixed key and the operation it runs in a context.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fixed {
    pub key: KeyPress,
    pub action: Action,
    /// A second way to press an operation that already has its own rebindable key (the arrows are
    /// `j`/`k`/`h`/`l`, `Home`/`End` are `g`/`G`). Aliases are not listed in the help or the footer.
    pub alias: bool,
}

/// The default operation of the fixed key `code` in `ctx`, or `None` when the key does nothing there.
/// Only the key code counts (modifiers are ignored, as they always were for these keys).
pub fn fixed_action(ctx: Context, code: KeyCode) -> Option<Action> {
    fixed_bindings(ctx)
        .into_iter()
        .find(|f| f.key.code == code)
        .map(|f| f.action)
}

/// Whether `code` is one of the keys that never reach the keymap (`keymap::is_fixed_key`).
pub fn is_fixed_code(code: KeyCode) -> bool {
    matches!(
        code,
        KeyCode::Up
            | KeyCode::Down
            | KeyCode::Left
            | KeyCode::Right
            | KeyCode::Home
            | KeyCode::End
            | KeyCode::Esc
            | KeyCode::Enter
            | KeyCode::Tab
            | KeyCode::BackTab
    )
}

/// Every fixed key of `ctx` with its operation (`Context::all()` is the set of contexts that use
/// the keymap, i.e. the ones that have fixed keys).
pub fn fixed_bindings(ctx: Context) -> Vec<Fixed> {
    let alias = |code, m| Fixed {
        key: KeyPress::key(code),
        action: Action::Navigate(m),
        alias: true,
    };
    let own = |code, action| Fixed {
        key: KeyPress::key(code),
        action,
        alias: false,
    };
    // A canvas has nothing to move: the navigation fixed keys (arrows, `Home` / `End`, `Enter`,
    // `Tab`) are the plugin's (`handle_fixed_key` runs its action on one and otherwise does
    // nothing), so the host keeps only `Esc` (`keys::reserved("canvas")`).
    if ctx == Context::Canvas {
        return vec![own(KeyCode::Esc, Action::Cancel)];
    }
    let mut v = vec![
        alias(KeyCode::Up, Motion::Up),
        alias(KeyCode::Down, Motion::Down),
        alias(KeyCode::Left, Motion::Left),
        alias(KeyCode::Right, Motion::Right),
        alias(KeyCode::Home, Motion::Top),
        alias(KeyCode::End, Motion::Bottom),
        own(KeyCode::Esc, Action::Cancel),
    ];
    if let Some(a) = enter_action(ctx) {
        v.push(own(KeyCode::Enter, a));
    }
    if matches!(ctx, Context::Text | Context::Document) {
        v.push(own(KeyCode::Tab, Action::LinkFocusNext));
        v.push(own(KeyCode::BackTab, Action::LinkFocusPrev));
    }
    // `Tab` / `Shift-Tab` walk the version-control screens: changes, log, graph, branches,
    // worktrees (the commit detail and the branch panel are not stops of the cycle).
    #[cfg(feature = "git")]
    if matches!(
        ctx,
        Context::Screen(
            _,
            Screen::Changes | Screen::Log | Screen::Graph | Screen::Branches | Screen::Worktrees
        )
    ) {
        v.push(own(KeyCode::Tab, Action::GitScreenNext));
        v.push(own(KeyCode::BackTab, Action::GitScreenPrev));
    }
    v
}

/// What `Enter` does in `ctx` (`None` = nothing).
fn enter_action(ctx: Context) -> Option<Action> {
    Some(match ctx {
        Context::Tree => Action::TreeActivate,
        Context::Text | Context::Document => Action::LinkOpen,
        Context::Bookmarks => Action::BookmarkJump,
        Context::Tabs => Action::TabListActivate,
        Context::Outline => Action::OutlineJump,
        // The grid opens the full-cell popup and the popup closes itself (the same toggle).
        Context::Table | Context::TableCell => Action::ToggleTableCell,
        #[cfg(feature = "git")]
        Context::Screen(_, s) => match s {
            Screen::Changes => Action::GitOpenSelectedDiff,
            Screen::Log | Screen::Graph => Action::GitOpenDetail,
            Screen::GraphPicker => Action::GitGraphPickerApply,
            Screen::Branches => Action::BranchCheckout,
            Screen::Worktrees => Action::WorktreeGoto,
            Screen::Detail => return None,
        },
        _ => return None,
    })
}

/// Every key that runs `action` in `ctx`: the keymap's keys (config applied) followed by the fixed
/// keys (aliases left out).
pub fn keys_of(app: &App, ctx: Context, action: Action) -> Vec<KeyPress> {
    // A plugin's operation has the one key of the plugin's layer.
    if let Action::Plugin(p) = action {
        return app.plugin_key_of(ctx, p).into_iter().collect();
    }
    let mut v = app.keymaps.keys_of(ctx, action);
    for f in fixed_bindings(ctx) {
        if f.action == action && !f.alias && !v.contains(&f.key) {
            v.push(f.key);
        }
    }
    v
}

/// The keys of an operation group as columns (`konoma_host_core::help::zip_columns`: a group of
/// exactly two operations is zipped, anything else is one key per column).
pub fn columns(app: &App, ctx: Context, acts: &[Action]) -> (Vec<Vec<KeyPress>>, bool) {
    konoma_host_core::help::zip_columns(acts, |a| keys_of(app, ctx, a))
}

pub use konoma_host_core::help::{footer_key, help_key};

/// Whether the screens of `ctx` write `Enter` as `↵` in the footer.
pub fn short_enter(ctx: Context) -> bool {
    matches!(
        ctx,
        Context::Text | Context::Document | Context::Image | Context::Table | Context::Canvas
    )
}
