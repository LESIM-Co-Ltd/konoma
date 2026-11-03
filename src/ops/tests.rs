// Tests of the operation declarations: the help and the footer are built from the keymap, so
// (1) the keys they print are the keys bound to the operation they name, (2) rebinding changes
// them, (3) they show only what acts (`gate`), and (4) every operation is declared and every bound
// operation is listed somewhere.

use std::collections::HashMap;

use super::decl::{decl, text};
use super::footer::{plan, render_tokens, Foot};
use super::gate::{gate, Gate};
use super::help::{listed_actions, rows_for};
use super::keys::{fixed_action, fixed_bindings, keys_of};
use crate::app::App;
use crate::config::Config;
use crate::i18n::Msg;
use crate::keymap::{Action, Binding, Context, KeyMap, KeyPress, KeyScheme, Motion, Resolution};
#[cfg(feature = "git")]
use crate::keymap::{Backend, Screen};
use crate::test_support::{unique_tmp, TmpDir};

fn app_with(cfg: Config) -> (App, TmpDir) {
    let dir = unique_tmp("konoma_ops_tests");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.txt"), b"hello\n").unwrap();
    (App::new(dir.to_path_buf(), cfg).unwrap(), dir)
}

fn app() -> (App, TmpDir) {
    app_with(Config::default())
}

/// A config with `table` (a `[keys.<table>]` name) holding the given `key = action` pairs.
fn cfg_keys(table: &str, pairs: &[(&str, &str)]) -> Config {
    let mut cfg = Config::default();
    let mut t = HashMap::new();
    for (k, a) in pairs {
        t.insert((*k).to_string(), (*a).to_string());
    }
    cfg.keys.surfaces.insert(table.to_string(), t);
    cfg
}

fn config_with_scheme(scheme: &str) -> Config {
    let mut cfg = Config::default();
    cfg.ui.keys = scheme.to_string();
    cfg
}

/// What a key can run in `ctx`: the fixed key's default operation (the key handler tries it
/// first; `Space` only while it acts) and the keymap's.
fn actions_of(app: &App, ctx: Context, kp: KeyPress) -> Vec<Action> {
    let mut v = Vec::new();
    if let Some(a) = fixed_action(ctx, kp.code) {
        v.push(a);
    }
    if let Resolution::Action(a) = app.keymaps.resolve(ctx, None, kp) {
        v.push(a);
    }
    v
}

/// Every action a plan names, whether or not it acts right now.
fn plan_actions(plan: &[Foot]) -> Vec<Action> {
    let mut v = Vec::new();
    for f in plan {
        match f {
            Foot::Op(a) | Foot::As(a, _) => v.push(*a),
            Foot::Group(acts, _) | Foot::GroupOp(acts) | Foot::Union(acts, _) => {
                v.extend(acts.iter().copied())
            }
            Foot::Plugin(p) => v.push(Action::Plugin(*p)),
            Foot::Core(c) => v.extend(
                c.ops()
                    .into_iter()
                    .filter_map(crate::ops::core_help::action_of),
            ),
            Foot::Leader(..) | Foot::Dyn(_) | Foot::HelpOverlay => {}
        }
    }
    v
}

// ---------------------------------------------------------------------------------------------
// (4) Every operation is declared
// ---------------------------------------------------------------------------------------------

/// Operations that are never named by a footer token or a help row of their own: menu items (the
/// `y` and `Space` menus list them), the fixed keys' defaults that the key itself names, and
/// operations with no key by default.
fn is_named_elsewhere(a: Action) -> bool {
    use Action as A;
    #[allow(unused_mut)]
    let mut named = matches!(
        a,
        A::Noop
            | A::TabGoto(_)
            | A::TabClose
            | A::CopyPath(_)
            | A::CopyCodeBlock
            | A::TableCopy(_)
            | A::FileCreate
            | A::FileRename
            | A::FileDelete
            | A::FileCopy
            | A::FileCut
            | A::FilePaste
            | A::FileDuplicate
            | A::Cancel
            | A::TabListActivate
            | A::OutlineJump
            | A::TabListClose
            | A::BookmarkJump
            | A::BookmarkEdit
            | A::BookmarkDelete
            | A::BookmarkClose
            | A::LinkOpen
    );
    #[cfg(feature = "git")]
    {
        named |= matches!(a, A::GitCopy(_) | A::CopyBranchName);
    }
    named
}

#[test]
fn every_action_in_the_default_keymaps_is_declared() {
    // `decl` is an exhaustive match, so a new `Action` does not compile until it has an entry;
    // this checks the entries are not empty for anything that has a key.
    for scheme in [KeyScheme::Vim, KeyScheme::Less] {
        let km = KeyMap::defaults(scheme);
        let mut actions: Vec<Action> = Vec::new();
        let mut grab = |m: &HashMap<KeyPress, Binding>| {
            for b in m.values() {
                if let Binding::Run(a) = b {
                    actions.push(*a);
                }
            }
        };
        km.contexts.values().for_each(&mut grab);
        km.types.values().for_each(&mut grab);
        grab(&km.basic);
        grab(&km.global);
        for menu in km.leaders.values() {
            actions.extend(menu.items.iter().map(|it| it.action));
        }
        for ctx in Context::all() {
            actions.extend(fixed_bindings(ctx).into_iter().map(|f| f.action));
        }
        assert!(
            actions.len() > 100,
            "the default keymap collected too little"
        );
        for a in actions {
            let d = decl(a);
            assert!(
                d.label.is_some() || d.help.is_some() || is_named_elsewhere(a),
                "{a:?} is bound to a key but declares no label and no description"
            );
        }
    }
}

// ---------------------------------------------------------------------------------------------
// (1) The keys printed are the keys bound to the operation
// ---------------------------------------------------------------------------------------------

fn check_tokens_and_rows(app: &App) {
    for ctx in Context::all() {
        for t in render_tokens(app, ctx, &plan(app, ctx)) {
            for k in &t.keys {
                let runs = actions_of(app, ctx, *k);
                if runs.is_empty() {
                    // A leader's opening key resolves to the menu, not to an action.
                    assert!(
                        matches!(
                            app.keymaps.resolve(ctx, None, *k),
                            Resolution::EnterLeader(_)
                        ),
                        "{ctx:?}: token {:?} prints {k:?}, which is bound to nothing",
                        t.text
                    );
                    continue;
                }
                let Some(a) = runs.iter().find(|a| t.actions.contains(a)) else {
                    panic!(
                        "{ctx:?}: token {:?} prints {k:?}, which runs {runs:?}, not one of {:?}",
                        t.text, t.actions
                    );
                };
                // Never print a key whose operation does not act (the gate is shared).
                assert!(
                    super::gate::action_enabled(app, ctx, *a),
                    "{ctx:?}: token {:?} offers {a:?}, which does not act now",
                    t.text
                );
            }
        }
        for row in rows_for(app, ctx) {
            for k in &row.keys {
                let runs = actions_of(app, ctx, *k);
                let Some(a) = runs.iter().find(|a| row.actions.contains(a)) else {
                    panic!(
                        "{ctx:?}: help row {:?} prints {k:?}, which runs {runs:?}, not one of {:?}",
                        row.keys_text, row.actions
                    );
                };
                if !row.ungated {
                    assert!(
                        super::gate::action_enabled(app, ctx, *a),
                        "{ctx:?}: help row {:?} lists {a:?}, which does not act now",
                        row.keys_text
                    );
                }
            }
        }
    }
}

#[test]
fn footer_and_help_keys_are_the_keys_bound_to_the_operation() {
    for scheme in ["vim", "less"] {
        let (app, _d) = app_with(config_with_scheme(scheme));
        check_tokens_and_rows(&app);
    }
}

#[test]
fn the_notation_follows_a_rebound_keymap_too() {
    // The same check on a keymap the user changed: moved, added, and removed keys.
    let mut cfg = cfg_keys(
        "tree",
        &[(",", "toggle_hidden"), (".", "noop"), ("x", "tree_leave")],
    );
    cfg.keys.surfaces.insert(
        "text".into(),
        [("z".to_string(), "search_start".to_string())]
            .into_iter()
            .collect(),
    );
    let (app, _d) = app_with(cfg);
    check_tokens_and_rows(&app);
}

// ---------------------------------------------------------------------------------------------
// (2) Rebinding changes the notation
// ---------------------------------------------------------------------------------------------

fn tokens_of(app: &App, ctx: Context) -> Vec<String> {
    render_tokens(app, ctx, &plan(app, ctx))
        .into_iter()
        .map(|t| t.text)
        .collect()
}

fn help_text(app: &App, ctx: Context) -> Vec<(String, String)> {
    super::help::sections_for(app, ctx)
        .into_iter()
        .flat_map(|s| s.rows)
        .collect()
}

#[test]
fn rebinding_a_key_changes_the_footer_and_the_help() {
    let (before, _d1) = app();
    let hidden = crate::i18n::tr(before.lang, Msg::HintHidden);
    assert!(
        tokens_of(&before, Context::Tree).contains(&format!(".:{hidden}")),
        "default: .:hidden"
    );

    let (after, _d2) = app_with(cfg_keys("tree", &[("H", "toggle_hidden"), (".", "noop")]));
    let toks = tokens_of(&after, Context::Tree);
    assert!(toks.contains(&format!("H:{hidden}")), "{toks:?}");
    assert!(!toks.contains(&format!(".:{hidden}")), "{toks:?}");
    let desc = crate::i18n::tr(after.lang, Msg::ToggleHidden);
    let rows = help_text(&after, Context::Tree);
    assert!(
        rows.iter().any(|(k, d)| k == "H" && d == desc),
        "help row for the moved key: {rows:?}"
    );
    assert!(
        !rows.iter().any(|(k, d)| k == "." && d == desc),
        "the old key must be gone: {rows:?}"
    );

    // An operation with two keys lists both (`/` and the added `z`).
    let (two, _d3) = app_with(cfg_keys("text", &[("z", "search_start")]));
    let search = crate::i18n::tr(two.lang, Msg::HintSearch);
    let toks = tokens_of(&two, Context::Text);
    assert!(toks.contains(&format!("//z:{search}")), "{toks:?}");
}

#[test]
fn a_menu_key_follows_the_config_in_the_help() {
    let (app, _d) = app_with(cfg_keys("global", &[("y q", "copy_name")]));
    let rows = help_text(&app, Context::Tree);
    assert!(
        rows.iter().any(|(k, _)| k == "y q"),
        "the copy menu lists the added item: {rows:?}"
    );
}

#[cfg(feature = "git")]
#[test]
fn a_version_control_key_follows_the_config() {
    let (app, _d) = app_with(cfg_keys(
        "git.changes",
        &[("a", "git_stage"), ("s", "noop")],
    ));
    let ctx = Context::Screen(Backend::Git, Screen::Changes);
    let stage = crate::i18n::tr(app.lang, Msg::FtStage);
    // Whether the writes act depends on the backend of the directory; ask the gate, not a repo.
    let toks = tokens_of(&app, ctx);
    let has = toks.iter().any(|t| t == &format!("a/S:{stage}"));
    let write = crate::vcs::caps(&app.tab.root).write;
    assert_eq!(has, write, "{toks:?}");
    assert!(!toks.iter().any(|t| t.starts_with("s/S")), "{toks:?}");
}

// ---------------------------------------------------------------------------------------------
// (3) The gate
// ---------------------------------------------------------------------------------------------

#[test]
fn a_hint_for_an_operation_that_does_not_act_is_not_shown() {
    let (mut app, _d) = app();
    // A plain directory, scanned: not a repository.
    app.ensure_git_status_now();
    assert_eq!(
        gate(&app, Context::Tree, Action::OpenGitView),
        Gate::Off(Some(Msg::NotAGitRepo))
    );
    let git = crate::i18n::tr(app.lang, Msg::HintGit);
    assert!(
        !tokens_of(&app, Context::Tree)
            .iter()
            .any(|t| t == &format!("p:{git}")),
        "p:git is offered outside a repository"
    );
    assert!(
        !help_text(&app, Context::Tree).iter().any(|(k, _)| k == "p"),
        "the help lists p outside a repository"
    );
}

#[cfg(feature = "git")]
#[test]
fn a_repository_offers_the_hub_and_a_changed_file_its_diff() {
    use crate::test_support::{init_git_repo, run_git};
    let dir = unique_tmp("konoma_ops_repo");
    std::fs::create_dir_all(&dir).unwrap();
    init_git_repo(&dir);
    std::fs::write(dir.join("a.txt"), b"one\n").unwrap();
    run_git(&dir, &["add", "-A"]);
    run_git(&dir, &["commit", "-q", "-m", "init"]);
    std::fs::write(dir.join("b.txt"), b"two\n").unwrap();
    let mut app = App::new(dir.canonicalize().unwrap(), Config::default()).unwrap();
    app.ensure_git_status_now();
    assert_eq!(gate(&app, Context::Tree, Action::OpenGitView), Gate::On);
    let git = crate::i18n::tr(app.lang, Msg::HintGit);
    assert!(tokens_of(&app, Context::Tree).contains(&format!("p:{git}")));

    // The cursor decides whether `d` has a diff to open.
    let sel = |app: &mut App, name: &str| {
        app.tab.selected = app
            .tab
            .entries
            .iter()
            .position(|e| e.path.ends_with(name))
            .unwrap();
    };
    sel(&mut app, "a.txt");
    assert_eq!(
        gate(&app, Context::Tree, Action::OpenGitDiffCursor),
        Gate::Off(Some(Msg::NoChanges))
    );
    sel(&mut app, "b.txt");
    assert_eq!(
        gate(&app, Context::Tree, Action::OpenGitDiffCursor),
        Gate::On
    );
    let diff = crate::i18n::tr(app.lang, Msg::HintDiff);
    assert!(tokens_of(&app, Context::Tree).contains(&format!("d:{diff}")));
}

#[cfg(feature = "git")]
#[test]
fn a_read_only_backend_is_refused_every_write_by_the_gate() {
    use crate::test_support::jj_scratch_seeded;
    let Some(dir) = jj_scratch_seeded("konoma_ops_jj_gate") else {
        return;
    };
    let app = App::new(dir.to_path_buf(), Config::default()).unwrap();
    for (s, a) in [
        (Screen::Branches, Action::BranchCheckout),
        (Screen::Branches, Action::BranchCreate),
        (Screen::Changes, Action::GitStage),
        (Screen::Changes, Action::GitCommit),
        (Screen::Worktrees, Action::WorktreeCreate),
    ] {
        assert_eq!(
            gate(&app, Context::Screen(Backend::Jj, s), a),
            Gate::Off(Some(Msg::VcsReadOnly)),
            "{a:?}"
        );
    }
    // Reading is allowed, and jj's own sync only exists on jj.
    assert_eq!(
        gate(
            &app,
            Context::Screen(Backend::Jj, Screen::Changes),
            Action::JjSync
        ),
        Gate::On
    );
    assert_eq!(
        gate(
            &app,
            Context::Screen(Backend::Git, Screen::Changes),
            Action::JjSync
        ),
        Gate::Off(Some(Msg::JjSyncNotJj))
    );
}

#[test]
fn page_keys_act_only_where_the_preview_has_pages() {
    // A text file has no pages: `J`/`K` do not act, so they are neither offered nor listed.
    let (mut app, _d) = app();
    app.tab.selected = app
        .tab
        .entries
        .iter()
        .position(|e| e.path.ends_with("a.txt"))
        .unwrap();
    app.tree_activate().unwrap();
    assert_eq!(gate(&app, Context::Text, Action::PageNext), Gate::Off(None));
    let toks = tokens_of(&app, Context::Text);
    assert!(
        !toks.iter().any(|t| t.starts_with("J/K")),
        "J/K offered on a text file: {toks:?}"
    );
}

// ---------------------------------------------------------------------------------------------
// The fixed keys
// ---------------------------------------------------------------------------------------------

#[test]
fn fixed_keys_have_a_default_operation_per_context() {
    use crossterm::event::KeyCode as K;
    for ctx in Context::all() {
        assert_eq!(fixed_action(ctx, K::Esc), Some(Action::Cancel), "{ctx:?}");
        // A canvas has nothing to move: the arrows are the plugin's, so the host binds none.
        let up = (ctx != Context::Canvas).then_some(Action::Navigate(Motion::Up));
        assert_eq!(fixed_action(ctx, K::Up), up, "{ctx:?}");
    }
    let enter = |c| fixed_action(c, K::Enter);
    assert_eq!(enter(Context::Tree), Some(Action::TreeActivate));
    assert_eq!(enter(Context::Text), Some(Action::LinkOpen));
    assert_eq!(enter(Context::Document), Some(Action::LinkOpen));
    assert_eq!(enter(Context::Table), Some(Action::ToggleTableCell));
    assert_eq!(enter(Context::TableCell), Some(Action::ToggleTableCell));
    assert_eq!(enter(Context::Bookmarks), Some(Action::BookmarkJump));
    assert_eq!(enter(Context::Tabs), Some(Action::TabListActivate));
    assert_eq!(enter(Context::Outline), Some(Action::OutlineJump));
    assert_eq!(enter(Context::Image), None);
    assert_eq!(
        fixed_action(Context::Document, K::Tab),
        Some(Action::LinkFocusNext)
    );
    assert_eq!(
        fixed_action(Context::Document, K::BackTab),
        Some(Action::LinkFocusPrev)
    );
    // `Space` is not a fixed key any more: `Enter` toggles the focused checkbox.
    assert_eq!(fixed_action(Context::Document, K::Char(' ')), None);
    assert_eq!(fixed_action(Context::Table, K::Tab), None);
    #[cfg(feature = "git")]
    for b in Backend::ALL {
        let e = |s| fixed_action(Context::Screen(b, s), K::Enter);
        assert_eq!(e(Screen::Changes), Some(Action::GitOpenSelectedDiff));
        assert_eq!(e(Screen::Log), Some(Action::GitOpenDetail));
        assert_eq!(e(Screen::Graph), Some(Action::GitOpenDetail));
        assert_eq!(e(Screen::GraphPicker), Some(Action::GitGraphPickerApply));
        assert_eq!(e(Screen::Branches), Some(Action::BranchCheckout));
        assert_eq!(e(Screen::Worktrees), Some(Action::WorktreeGoto));
        assert_eq!(e(Screen::Detail), None);
    }
}

#[test]
fn fixed_keys_are_listed_with_their_operation() {
    let (app, _d) = app();
    // `Enter` is a key of `tree_activate`; the arrows are aliases and are not listed.
    let keys = keys_of(&app, Context::Tree, Action::TreeActivate);
    assert_eq!(keys, vec![KeyPress::key(crossterm::event::KeyCode::Enter)]);
    let down = keys_of(&app, Context::Tree, Action::Navigate(Motion::Down));
    assert_eq!(down, vec![KeyPress::ch('j')]);
}

// ---------------------------------------------------------------------------------------------
// (4b) Every bound operation is documented
// ---------------------------------------------------------------------------------------------

/// Layer-0 operations every screen lists through the shared global rows.
fn is_global_listed(a: Action) -> bool {
    matches!(
        a,
        Action::TabNew
            | Action::TabNewAfter
            | Action::TabPrev
            | Action::TabNext
            | Action::ToggleTabList
            | Action::PasteJump
            | Action::ToggleHelp
            | Action::ToggleFollow
            | Action::Quit
    )
}

/// The movement keys of the overlays and of the pickers: named by the footer as `j/k` and, on a
/// screen with no footer of its own, by the screen underneath.
fn is_overlay_nav(ctx: Context, a: Action) -> bool {
    let overlay = matches!(
        ctx,
        Context::Bookmarks
            | Context::Tabs
            | Context::Outline
            | Context::Info
            | Context::Help
            | Context::TableCell
            | Context::TextVisual
            | Context::TreeVisual
    );
    overlay
        && matches!(
            a,
            Action::Navigate(_)
                | Action::ListFilterStart
                | Action::BookmarkClose
                | Action::ToggleTabList
                | Action::ToggleOutline
                | Action::InfoClose
        )
}

#[test]
fn every_bound_operation_is_listed_in_the_help_or_the_footer() {
    // The reverse of the notation check: a key added to a table must not go unlisted. The help
    // lists the screens' operations; the overlays (sort menu, bookmark list, ...) have no help of
    // their own and name theirs in the footer.
    let mut missing: Vec<String> = Vec::new();
    for scheme in ["vim", "less"] {
        let (app, _d) = app_with(config_with_scheme(scheme));
        for ctx in Context::all() {
            let mut listed = listed_actions(&app, ctx);
            listed.extend(plan_actions(&plan(&app, ctx)));
            if ctx == Context::Tree {
                listed.extend(plan_actions(&super::footer::changed_filter_plan()));
            }
            // The text and document screens share one keymap table, and each lists what the other
            // does not (the focused-item keys belong to the document, the raw-source toggle to both).
            if ctx == Context::Text || ctx == Context::Document {
                listed.extend(plan_actions(&plan(&app, Context::Document)));
                listed.extend(plan_actions(&plan(&app, Context::Text)));
                listed.extend(listed_actions(&app, Context::Document));
                listed.extend(listed_actions(&app, Context::Text));
            }
            let mut bound: Vec<Action> = Vec::new();
            let mut grab = |m: &HashMap<KeyPress, Binding>| {
                for (k, b) in m {
                    if let Binding::Run(a) = b {
                        if app.keymaps.resolve(ctx, None, *k) == Resolution::Action(*a) {
                            bound.push(*a);
                        }
                    }
                }
            };
            if let Some(m) = app.keymaps.contexts.get(&ctx) {
                grab(m);
            }
            for vt in ctx.view_types() {
                if let Some(m) = app.keymaps.types.get(vt) {
                    grab(m);
                }
            }
            if ctx.inherits_basic() {
                grab(&app.keymaps.basic);
            }
            for a in bound {
                if is_named_elsewhere(a) || listed.contains(&a) || is_global_listed(a) {
                    continue;
                }
                // Each backend's screen lists only what its backend does: git's graph already
                // shows every commit, and pinning a base is git's (`ops::gate`, `ops::footer`).
                #[cfg(feature = "git")]
                if matches!(
                    (ctx, a),
                    (Context::Screen(Backend::Git, _), Action::GitGraphToggleAll)
                        | (
                            Context::Screen(Backend::Jj, _),
                            Action::GitGraphSetBase | Action::GitGraphClearBase
                        )
                ) {
                    continue;
                }
                if !is_overlay_nav(ctx, a) {
                    missing.push(format!("{scheme}/{ctx:?}: {a:?}"));
                }
            }
        }
    }
    missing.sort();
    missing.dedup();
    assert!(
        missing.is_empty(),
        "bound to a key but neither the help nor the footer lists it:\n{}",
        missing.join("\n")
    );
}

// ---------------------------------------------------------------------------------------------
// The text of a declaration
// ---------------------------------------------------------------------------------------------

#[test]
fn a_label_that_depends_on_the_state_is_a_function_of_it() {
    let (mut app, _d) = app();
    // `q` in the tree is "quit" with one tab and "close tab" with two.
    assert_eq!(
        text(&app, Context::Tree, Action::CloseTabOrQuit).label,
        Some(Msg::HintQuit)
    );
    app.tab_new().unwrap();
    assert_eq!(
        text(&app, Context::Tree, Action::CloseTabOrQuit).label,
        Some(Msg::HintCloseTab)
    );
}

/// No two items of a `Space` menu carry the same label, in any context and either language: a
/// pair of operations (`[` / `]`, `J` / `K`, `Ctrl-n` / `Ctrl-p` ...) reads "tab", "tab" in the
/// footer, where the width is short and the keys name the direction, but a menu line has room to
/// say which one it is.
#[test]
fn space_menu_items_never_share_a_label() {
    let (mut app, _d) = app();
    let mut seen_pairs = 0;
    let mut clashes: Vec<String> = Vec::new();
    for lang in [crate::i18n::Lang::En, crate::i18n::Lang::Jp] {
        app.lang = lang;
        for ctx in Context::all() {
            let items = crate::ops::menu::declared_actions_menu(&app, ctx);
            for (i, a) in items.iter().enumerate() {
                for b in &items[i + 1..] {
                    seen_pairs += 1;
                    if a.label == b.label {
                        clashes.push(format!(
                            "{lang:?} {ctx:?}: {:?} and {:?} are both `{}`",
                            a.action, b.action, a.label
                        ));
                    }
                }
            }
        }
    }
    assert!(clashes.is_empty(), "{}", clashes.join("\n"));
    assert!(
        seen_pairs > 100,
        "the check looked at too little: {seen_pairs}"
    );
}

/// The help popup is 64 cells wide inside (`ui::help::render`) and does not wrap, so a longer row
/// is cut. These descriptions were written for the stage-2 keys (`/ n N` over changes, `Tab` over
/// the Git screens, `o`) and must fit; the older long rows (`Enter` on a document and others) were
/// already cut in v0.31.0 and are not pinned here.
#[test]
fn the_help_rows_of_the_new_keys_fit_the_popup() {
    use unicode_width::UnicodeWidthStr;
    let (mut app, _d) = app();
    let mut checked = 0;
    for lang in [crate::i18n::Lang::En, crate::i18n::Lang::Jp] {
        app.lang = lang;
        let watched: Vec<String> = [
            Msg::SearchHint,
            Msg::GitScreensHelp,
            Msg::JjScreensHelp,
            Msg::OpenAppHelp,
        ]
        .iter()
        .map(|m| crate::i18n::tr(lang, *m).to_string())
        .collect();
        for ctx in Context::all() {
            for sec in crate::ops::help::sections_for(&app, ctx) {
                for (k, d) in &sec.rows {
                    if !watched.contains(d) {
                        continue;
                    }
                    checked += 1;
                    let pad = if k.chars().count() >= 16 { 2 } else { 0 };
                    let w = 2 + 16.max(k.chars().count()) + pad + d.width();
                    assert!(w <= 64, "{lang:?} {ctx:?}: `{k}` | {d} is {w} cells");
                }
            }
        }
    }
    assert!(checked >= 10, "the check looked at too little: {checked}");
}
