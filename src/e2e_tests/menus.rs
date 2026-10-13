//! The three menus (plugin stage 2 / B3b): `,` sorts and shows, `Space` lists the operations of the
//! screen, `y` copies — each with the content of the context it is opened in. Driven through the
//! real `handle_key`.

use super::*;
use crate::keymap::{Action, Context, LeaderId};
use crate::ops::menu::{menu, View};

fn items(s: &Sim, ctx: Context, id: LeaderId) -> Vec<(String, Action)> {
    menu(&s.app, ctx, id, View::Open)
        .map(|m| {
            m.items
                .into_iter()
                .map(|i| (i.key.label(), i.action))
                .collect()
        })
        .unwrap_or_default()
}

fn keys_of(v: &[(String, Action)]) -> Vec<&str> {
    v.iter().map(|(k, _)| k.as_str()).collect()
}

/// `,` in the tree holds the old sort menu's letters plus the path style and the hidden files,
/// and the letters do what they did.
#[test]
fn e2e_options_menu_in_the_tree_sorts_and_shows() {
    let dir = sandbox("menus_options_tree");
    seed_files(&dir);
    let mut s = Sim::new(&canon(&dir));
    let it = items(&s, Context::Tree, LeaderId::Options);
    assert_eq!(
        keys_of(&it),
        ["n", "s", "m", "e", "r", ".", "p", "h"],
        "{it:?}"
    );
    assert_eq!(it[0].1, Action::SortSet(crate::app::SortKey::Name));
    assert_eq!(it[4].1, Action::SortToggleReverse);
    assert_eq!(it[5].1, Action::SortToggleDirsFirst);
    assert_eq!(it[6].1, Action::CyclePathStyle);
    assert_eq!(it[7].1, Action::ToggleHidden);

    s.key(',');
    assert_eq!(s.app.pending_leader, Some(LeaderId::Options));
    s.see("Sort & display");
    s.key('s');
    assert_eq!(s.app.pending_leader, None);
    assert_eq!(s.app.sort.key, crate::app::SortKey::Size);
    assert!(
        !s.app.is_sort_menu(),
        "the menu does not turn the old overlay on"
    );
    s.key(',');
    s.key('r');
    assert!(s.app.sort.reverse);
    assert!(
        !s.app.is_sort_menu(),
        "a toggle does not open the old overlay either"
    );
    s.key(',');
    s.key('.');
    assert!(!s.app.sort.dirs_first);
    let style = s.app.path_style;
    s.key(',');
    s.key('p');
    assert_ne!(s.app.path_style, style);
    s.dont_see(".hidden.txt");
    s.key(',');
    s.key('h');
    s.see(".hidden.txt");
}

/// `,` in a preview wraps lines and picks the path style; a table and an image only the path
/// style; and the old tree keys `s` / `p` are free.
#[test]
fn e2e_options_menu_in_the_previews() {
    let dir = sandbox("menus_options_preview");
    seed_files(&dir);
    let mut s = Sim::new(&canon(&dir));

    s.select("notes.txt");
    s.enter();
    assert_eq!(s.app.key_context(), Some(Context::Text));
    assert_eq!(
        keys_of(&items(&s, Context::Text, LeaderId::Options)),
        ["p", "w"]
    );
    let wrap = s.app.cfg.ui.wrap;
    s.key(',');
    assert_eq!(s.app.pending_leader, Some(LeaderId::Options));
    s.key('w');
    assert_ne!(s.app.cfg.ui.wrap, wrap, "w flips the wrapping");
    s.key(',');
    s.key('w');
    assert_eq!(s.app.cfg.ui.wrap, wrap);
    let style = s.app.path_style;
    s.key('p');
    assert_eq!(
        s.app.path_style, style,
        "p is not the path style in a preview any more"
    );
    s.key(',');
    s.key('p');
    assert_ne!(s.app.path_style, style);
    s.key('q');

    s.select("readme.md");
    s.enter();
    assert_eq!(s.app.key_context(), Some(Context::Document));
    assert_eq!(
        keys_of(&items(&s, Context::Document, LeaderId::Options)),
        ["p", "w"]
    );
    let wrap = s.app.cfg.ui.wrap;
    s.key(',');
    s.key('w');
    assert_ne!(s.app.cfg.ui.wrap, wrap);
    s.key('q');

    s.select("data.csv");
    s.enter();
    assert_eq!(s.app.key_context(), Some(Context::Table));
    // Wrapping is the text's and the document's: a table is offered the path style alone.
    assert_eq!(
        keys_of(&items(&s, Context::Table, LeaderId::Options)),
        ["p"]
    );
    assert_eq!(
        keys_of(&items(&s, Context::Image, LeaderId::Options)),
        ["p"]
    );
    s.key(',');
    assert_eq!(s.app.pending_leader, Some(LeaderId::Options));
    s.esc();
}

/// `,` in a diff: the follow scope (only while it means something) and the path style.
#[cfg(feature = "git")]
#[test]
fn e2e_options_menu_in_a_diff() {
    let dir = sandbox("menus_options_diff");
    seed_repo(&dir);
    let mut s = Sim::new(&canon(&dir));
    s.select("a.rs");
    s.key('d');
    assert!(s.app.is_git_diff_preview());
    assert_eq!(s.app.key_context(), Some(Context::Diff));
    // The keymap's menu holds both; the follow scope is offered only while a follow diff is open.
    let declared: Vec<_> = s
        .app
        .keymaps
        .menu(Context::Diff, LeaderId::Options)
        .unwrap()
        .items
        .iter()
        .map(|i| i.action)
        .collect();
    assert_eq!(
        declared,
        [Action::ToggleFollowDiffScope, Action::CyclePathStyle]
    );
    assert_eq!(keys_of(&items(&s, Context::Diff, LeaderId::Options)), ["p"]);
    s.key(',');
    assert_eq!(s.app.pending_leader, Some(LeaderId::Options));
    s.key('p');
    assert_eq!(s.app.last_dispatched, Some(Action::CyclePathStyle));
}

/// Each context's `Space` menu: it lists operations that act, and choosing an item runs exactly
/// that operation.
#[test]
fn e2e_space_menu_lists_what_acts_and_runs_it() {
    type Setup = fn(&mut Sim);
    let scenarios: Vec<(&str, Setup)> = vec![
        ("text", |s| {
            s.select("notes.txt");
            s.enter();
        }),
        ("document", |s| {
            s.select("readme.md");
            s.enter();
        }),
        ("table", |s| {
            s.select("data.csv");
            s.enter();
        }),
        ("bookmarks", |s| {
            s.key('m');
            s.key('a');
            s.key('\'');
        }),
        ("tabs", |s| {
            s.key('T');
        }),
    ];
    for (name, setup) in scenarios {
        let dir = sandbox(&format!("menus_space_{name}"));
        seed_files(&dir);
        let mut s = Sim::new(&canon(&dir));
        setup(&mut s);
        let ctx = s.app.key_context().expect("a keymap context");
        let it = menu(&s.app, ctx, LeaderId::Actions, View::Open)
            .unwrap_or_else(|| panic!("{name}: no menu"));
        assert!(!it.items.is_empty(), "{name}: the Space menu is empty");
        // The letters are distinct, `q`/Space are never one (`q` closes a menu), and every item acts.
        let mut seen: Vec<String> = Vec::new();
        for i in &it.items {
            assert!(
                !seen.contains(&i.key.label()),
                "{name}: {} twice",
                i.key.label()
            );
            seen.push(i.key.label());
            assert_ne!(i.key.label(), "q", "{name}");
            assert!(
                crate::ops::action_enabled(&s.app, ctx, i.action),
                "{name}: {:?} is offered but does not act",
                i.action
            );
            assert!(!i.label.is_empty(), "{name}: {:?} has no label", i.action);
        }
        // Choosing each item runs that operation, from a fresh screen each time.
        for (n, want) in it.items.iter().enumerate() {
            let dir = sandbox(&format!("menus_space_{name}_{n}"));
            seed_files(&dir);
            let mut s = Sim::new(&canon(&dir));
            setup(&mut s);
            s.app.last_dispatched = None;
            s.key(' ');
            assert_eq!(s.app.pending_leader, Some(LeaderId::Actions), "{name}");
            s.press(want.key.code, crossterm::event::KeyModifiers::NONE);
            assert_eq!(s.app.pending_leader, None, "{name}");
            assert_eq!(
                s.app.last_dispatched,
                Some(want.action),
                "{name}: {} did not run {:?}",
                want.key.label(),
                want.action
            );
        }
    }
}

/// The operations that lost their key are still in the menu, and the bookmark list deletes with `d`.
#[test]
fn e2e_space_menu_reaches_operations_without_a_key() {
    let dir = sandbox("menus_space_nokey");
    seed_files(&dir);
    let mut s = Sim::new(&canon(&dir));
    s.select("notes.txt");
    s.enter();
    let it = items(&s, Context::Text, LeaderId::Actions);
    for want in [
        Action::SearchStart,
        Action::ToggleWrap,
        Action::CyclePathStyle,
    ] {
        assert!(it.iter().any(|(_, a)| *a == want), "{want:?} in {it:?}");
    }
    // Rebinding an operation away from its key does not take it out of the menu.
    let mut cfg = cfg_en();
    cfg.keys.surfaces.insert(
        "text".into(),
        [("/".to_string(), "noop".to_string())]
            .into_iter()
            .collect(),
    );
    let mut s2 = Sim::with_config(&canon(&dir), cfg);
    s2.select("notes.txt");
    s2.enter();
    s2.key('/');
    assert_ne!(
        s2.app.surface(),
        crate::keymap::Surface::Search,
        "/ is unbound"
    );
    let it = items(&s2, Context::Text, LeaderId::Actions);
    assert!(it.iter().any(|(_, a)| *a == Action::SearchStart), "{it:?}");

    // The bookmark list: Space d deletes the selected bookmark, Ctrl-e stays an edit key.
    let mut s = Sim::new(&canon(&dir));
    s.key('m');
    s.key('a');
    s.key('\'');
    assert_eq!(s.app.key_context(), Some(Context::Bookmarks));
    let it = items(&s, Context::Bookmarks, LeaderId::Actions);
    assert!(
        it.contains(&("d".to_string(), Action::BookmarkDelete)),
        "{it:?}"
    );
    assert!(it.iter().any(|(_, a)| *a == Action::BookmarkEdit), "{it:?}");
    s.ctrl('d');
    assert!(
        s.app.bookmarks.get('a').is_some(),
        "Ctrl-d no longer deletes"
    );
    s.key(' ');
    s.key('d');
    assert!(s.app.bookmarks.get('a').is_none());
}

/// The tree's `Space` is the file menu it always was.
#[test]
fn e2e_space_in_the_tree_is_the_file_menu() {
    let dir = sandbox("menus_space_tree");
    seed_files(&dir);
    let mut s = Sim::new(&canon(&dir));
    s.key(' ');
    assert_eq!(s.app.pending_leader, Some(LeaderId::File));
    let it = items(&s, Context::Tree, LeaderId::File);
    assert_eq!(keys_of(&it), ["n", "r", "d", "c", "x", "p", "D"]);
    s.see("File");
}

/// `y` opens the copy menu of the context: the tree's paths, a table's cell/row/column, a commit's
/// fields, a branch's name.
#[cfg(feature = "git")]
#[test]
fn e2e_y_opens_a_different_menu_in_each_context() {
    let dir = sandbox("menus_y");
    seed_repo(&dir);
    std::fs::write(dir.join("t.csv"), "a,b\n1,2\n").unwrap();
    let mut s = Sim::new(&canon(&dir));
    let mut seen: Vec<Vec<String>> = Vec::new();
    let mut open = |s: &mut Sim, want: LeaderId| {
        s.key('y');
        assert_eq!(s.app.pending_leader, Some(want));
        let ctx = s.app.key_context().unwrap();
        seen.push(items(s, ctx, want).iter().map(|(k, _)| k.clone()).collect());
        s.esc();
        assert_eq!(s.app.pending_leader, None);
    };
    open(&mut s, LeaderId::Copy);
    s.select("t.csv");
    s.enter();
    open(&mut s, LeaderId::TableCopy);
    s.key('q');
    s.key('p');
    s.key('b');
    assert!(s.app.is_git_branches());
    open(&mut s, LeaderId::BranchCopy);
    s.key('h');
    s.app.open_git_log();
    assert!(s.app.is_git_log());
    open(&mut s, LeaderId::GitCopy);
    // Four contexts, four menus.
    for (i, a) in seen.iter().enumerate() {
        for b in &seen[i + 1..] {
            assert_ne!(a, b);
        }
    }
}

/// A chord in `[keys.<context>]` changes the menu of that context only; the old `[keys.global]`
/// spelling keeps reaching every context.
#[test]
fn e2e_a_menu_chord_in_the_config_reaches_the_context_it_is_written_in() {
    let dir = sandbox("menus_chord");
    seed_files(&dir);
    let full = canon(&dir).join("notes.txt").display().to_string();

    // `[keys.text] "y z"`: copies in a text preview, not in the tree.
    let mut cfg = cfg_en();
    cfg.keys.surfaces.insert(
        "text".into(),
        [("y z".to_string(), "copy_full".to_string())]
            .into_iter()
            .collect(),
    );
    let mut s = Sim::with_config(&canon(&dir), cfg);
    s.select("notes.txt");
    crate::test_support::clear_test_clipboard();
    s.key('y');
    s.key('z');
    assert_eq!(
        crate::test_support::get_test_clipboard(),
        None,
        "not in the tree"
    );
    s.enter();
    s.key('y');
    s.key('z');
    assert_eq!(
        crate::test_support::get_test_clipboard().as_deref(),
        Some(full.as_str()),
        "in the text preview"
    );

    // `[keys.global] "y z"` (the old way): the tree and the preview both.
    let mut cfg = cfg_en();
    cfg.keys.surfaces.insert(
        "global".into(),
        [("y z".to_string(), "copy_full".to_string())]
            .into_iter()
            .collect(),
    );
    let mut s = Sim::with_config(&canon(&dir), cfg);
    s.select("notes.txt");
    crate::test_support::clear_test_clipboard();
    s.key('y');
    s.key('z');
    assert_eq!(
        crate::test_support::get_test_clipboard().as_deref(),
        Some(full.as_str()),
        "global reaches the tree"
    );
    crate::test_support::clear_test_clipboard();
    s.enter();
    s.key('y');
    s.key('z');
    assert_eq!(
        crate::test_support::get_test_clipboard().as_deref(),
        Some(full.as_str()),
        "and the preview"
    );
}

/// A key that is not in the open menu closes it and does nothing else — `Esc` and `q` too: they
/// do not leave the screen, and `Q` does not quit.
#[test]
fn e2e_a_key_outside_the_menu_only_closes_it() {
    let dir = sandbox("menus_close");
    seed_files(&dir);
    let mut cfg = cfg_en();
    cfg.ui.confirm_quit = false;
    let mut s = Sim::with_config(&canon(&dir), cfg);
    s.select("notes.txt");
    s.enter();
    assert_eq!(s.app.tab.mode, Mode::Preview);
    for opener in [' ', ',', 'y'] {
        for closer in ['Z', 'q', 'Q', '/', 'j'] {
            s.key(opener);
            assert!(s.app.pending_leader.is_some(), "{opener:?}");
            // A key that *is* an item of this menu (`Q` and `/` are in `Space`'s) runs it.
            let lead = s.app.pending_leader.unwrap();
            let ctx = s.app.key_context().unwrap();
            if items(&s, ctx, lead)
                .iter()
                .any(|(k, _)| *k == closer.to_string())
            {
                s.esc();
                continue;
            }
            s.app.last_dispatched = None;
            s.key(closer);
            assert_eq!(s.app.pending_leader, None, "{opener:?} {closer:?}");
            assert_eq!(
                s.app.last_dispatched, None,
                "{opener:?} {closer:?} ran something"
            );
            assert!(!s.quit, "{opener:?} {closer:?} quit");
            assert_eq!(
                s.app.tab.mode,
                Mode::Preview,
                "{opener:?} {closer:?} went back"
            );
            assert!(s.app.preview_search_query().is_none());
        }
        s.key(opener);
        s.esc();
        assert_eq!(s.app.pending_leader, None);
        assert_eq!(
            s.app.tab.mode,
            Mode::Preview,
            "Esc after {opener:?} went back"
        );
    }
}

/// The help lists the three menus of the screen it was opened over.
#[test]
fn e2e_help_lists_the_menus_of_the_screen() {
    let dir = sandbox("menus_help");
    seed_files(&dir);
    let mut s = Sim::with_config_sized(&canon(&dir), cfg_en(), 140, 90);
    s.select("notes.txt");
    s.enter();
    s.key('?');
    let help = s.screen();
    for want in ["Copy path", "Sort & display", "Actions", ", w", "Space "] {
        assert!(help.contains(want), "help lacks {want:?}:\n{help}");
    }
}
