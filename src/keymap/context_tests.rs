//! Tests of the context model itself: names, the old names that are still read, the layers and
//! what a table can do to the layers below it. (What the default keys *are* is pinned by the
//! equivalence tests against the old keymap, and by the per-key tests in `keymap::tests`.)

use super::*;

fn cfg_of(tables: &[(&str, &[(&str, &str)])]) -> KeysFileConfig {
    let mut cfg = KeysFileConfig::default();
    for (name, entries) in tables {
        let t: HashMap<String, String> = entries
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        cfg.surfaces.insert((*name).to_string(), t);
    }
    cfg
}

fn at(m: &KeyMap, ctx: Context, c: char) -> Resolution {
    m.resolve(ctx, None, KeyPress::ch(c))
}

fn run(a: Action) -> Resolution {
    Resolution::Action(a)
}

#[test]
fn context_names_are_the_documented_ones() {
    let mut names: Vec<String> = Context::all().iter().map(|c| c.name()).collect();
    names.sort();
    #[allow(unused_mut)]
    let mut expected: Vec<&str> = vec![
        "bookmarks",
        "document",
        "help",
        "image",
        "info",
        "outline",
        "sort",
        "table",
        "table_cell",
        "tabs",
        "text",
        "text_visual",
        "tree",
        "tree_visual",
    ];
    #[cfg(feature = "git")]
    expected.extend([
        "diff",
        "git.branches",
        "git.changes",
        "git.detail",
        "git.graph",
        "git.graph_picker",
        "git.log",
        "git.worktrees",
        "jj.branches",
        "jj.changes",
        "jj.detail",
        "jj.graph",
        "jj.graph_picker",
        "jj.log",
        "jj.worktrees",
    ]);
    expected.sort_unstable();
    assert_eq!(names, expected);
}

#[test]
fn a_dotted_context_name_is_written_with_quotes() {
    assert_eq!(Context::Tree.header(), "[keys.tree]");
    #[cfg(feature = "git")]
    {
        assert_eq!(
            Context::Screen(Backend::Git, Screen::Changes).header(),
            "[keys.\"git.changes\"]"
        );
        assert_eq!(
            Context::Screen(Backend::Jj, Screen::GraphPicker).header(),
            "[keys.\"jj.graph_picker\"]"
        );
    }
}

#[test]
fn the_text_preview_is_text_or_document_by_what_is_shown() {
    assert_eq!(
        Context::of_surface(Surface::PreviewText, false, false),
        Some(Context::Text)
    );
    assert_eq!(
        Context::of_surface(Surface::PreviewText, true, false),
        Some(Context::Document)
    );
    // The flag refines only the text preview.
    assert_eq!(
        Context::of_surface(Surface::PreviewImage, true, true),
        Some(Context::Image)
    );
    assert_eq!(
        Context::of_surface(Surface::Tree, true, true),
        Some(Context::Tree)
    );
}

#[cfg(feature = "git")]
#[test]
fn a_version_control_screen_gets_its_backends_context() {
    for (sfc, screen) in [
        (Surface::GitChanges, Screen::Changes),
        (Surface::GitLog, Screen::Log),
        (Surface::GitGraph, Screen::Graph),
        (Surface::GitGraphPicker, Screen::GraphPicker),
        (Surface::GitBranches, Screen::Branches),
        (Surface::GitWorktrees, Screen::Worktrees),
        (Surface::GitDetail, Screen::Detail),
    ] {
        assert_eq!(
            Context::of_surface(sfc, false, false),
            Some(Context::Screen(Backend::Git, screen))
        );
        assert_eq!(
            Context::of_surface(sfc, false, true),
            Some(Context::Screen(Backend::Jj, screen))
        );
    }
    // The diff of one file is the same context for both backends.
    assert_eq!(
        Context::of_surface(Surface::PreviewGitDiff, false, true),
        Some(Context::Diff)
    );
}

#[test]
fn every_context_maps_back_to_the_surface_it_refines() {
    for ctx in Context::all() {
        // The surface a context refines maps to this context for some flags.
        let found = [(false, false), (false, true), (true, false), (true, true)]
            .iter()
            .any(|&(doc, jj)| Context::of_surface(ctx.surface(), doc, jj) == Some(ctx));
        assert!(found, "{ctx:?}");
    }
}

// --- layers --------------------------------------------------------------------------------

#[test]
fn layer_one_gives_j_and_k_to_every_context_with_a_cursor() {
    let m = KeyMap::defaults(KeyScheme::Vim);
    for ctx in Context::all() {
        let expect = ctx.inherits_basic();
        for (c, mo) in [('j', Motion::Down), ('k', Motion::Up)] {
            let got = at(&m, ctx, c);
            if expect {
                assert_eq!(got, run(Action::Navigate(mo)), "{ctx:?} {c}");
            } else {
                assert_eq!(got, Resolution::Unbound, "{ctx:?} {c}");
            }
        }
    }
    // The two that have no cursor.
    assert!(!Context::Sort.inherits_basic());
    assert!(!Context::Info.inherits_basic());
}

#[test]
fn the_layers_are_where_the_design_puts_them() {
    let m = KeyMap::defaults(KeyScheme::Vim);
    // Layer 1.
    // j, k and the operation menu on `Space`.
    assert_eq!(m.basic.len(), 3);
    // Layer 2: the scrolling keys live in the type tables, not in the contexts' own tables.
    assert!(m.types[&ViewType::Text].contains_key(&KeyPress::ch('g')));
    assert!(!m.contexts[&Context::Text].contains_key(&KeyPress::ch('g')));
    assert!(!m.contexts[&Context::TextVisual].contains_key(&KeyPress::ch('0')));
    assert!(m.types[&ViewType::Table].contains_key(&KeyPress::ch('$')));
    assert!(m.types[&ViewType::Image].contains_key(&KeyPress::ch('+')));
    #[cfg(feature = "git")]
    {
        assert!(m.types[&ViewType::Diff].contains_key(&KeyPress::ch('s')));
        assert!(m.types[&ViewType::List].contains_key(&KeyPress::ch('G')));
        // A text-type context that is not the preview shares the text layer.
        assert!(!m.contexts[&Context::Screen(Backend::Git, Screen::Detail)]
            .contains_key(&KeyPress::ch('l')));
    }
    // Every view type has a table, and every table belongs to a view type.
    for vt in ViewType::all() {
        assert!(m.types.contains_key(&vt), "{vt:?} has no layer-2 table");
    }
    assert_eq!(m.types.len(), ViewType::all().len());
    // A document looks into the text layer too.
    assert_eq!(
        Context::Document.view_types(),
        &[ViewType::Document, ViewType::Text]
    );
    assert_eq!(at(&m, Context::Document, 'G'), at(&m, Context::Text, 'G'));
}

#[test]
fn text_and_document_start_with_the_same_keys() {
    for scheme in [KeyScheme::Vim, KeyScheme::Less] {
        let m = KeyMap::defaults(scheme);
        assert_eq!(
            m.contexts[&Context::Text],
            m.contexts[&Context::Document],
            "{scheme:?}: B1 keeps the one text-preview table for both"
        );
    }
}

#[cfg(feature = "git")]
#[test]
fn git_and_jj_start_with_the_same_tables_but_for_the_changes_screens_s() {
    for scheme in [KeyScheme::Vim, KeyScheme::Less] {
        let m = KeyMap::defaults(scheme);
        for s in Screen::ALL {
            let mut git = m.contexts[&Context::Screen(Backend::Git, s)].clone();
            let mut jj = m.contexts[&Context::Screen(Backend::Jj, s)].clone();
            if s == Screen::Changes {
                // The one difference: `s` stages in git and syncs in jj.
                let s_key = KeyPress::ch('s');
                assert_eq!(git.remove(&s_key), Some(Binding::Run(Action::GitStage)));
                assert_eq!(jj.remove(&s_key), Some(Binding::Run(Action::JjSync)));
            }
            assert_eq!(git, jj, "{s:?}");
        }
    }
}

#[cfg(feature = "git")]
#[test]
fn a_context_can_unbind_a_key_its_layer_gives_and_global_still_applies() {
    let m = KeyMap::defaults(KeyScheme::Vim);
    // The hub and the log get `G` and `g` from the list layer.
    let hub = Context::Screen(Backend::Git, Screen::Changes);
    assert_eq!(at(&m, hub, 'G'), run(Action::Navigate(Motion::Bottom)));
    assert_eq!(at(&m, hub, 'g'), run(Action::Navigate(Motion::Top)));
    let log = Context::Screen(Backend::Git, Screen::Log);
    assert_eq!(at(&m, log, 'G'), run(Action::Navigate(Motion::Bottom)));
    // Unbinding in config hides the layer's binding...
    let cfg = cfg_of(&[("git.log", &[("G", "noop")])]);
    let c = KeyMap::from_config(KeyScheme::Vim, &cfg);
    assert_eq!(at(&c, log, 'G'), Resolution::Unbound);
    // ...in that context only.
    assert_eq!(
        at(&c, Context::Screen(Backend::Jj, Screen::Log), 'G'),
        run(Action::Navigate(Motion::Bottom))
    );
    assert!(c.warnings.is_empty() && c.conflicts.is_empty());
    // A key that a context does not own and `global` does stays global (as it did when a surface
    // table merely lacked it).
    let cfg = cfg_of(&[("tree", &[("1", "noop"), ("T", "noop")])]);
    let c = KeyMap::from_config(KeyScheme::Vim, &cfg);
    assert_eq!(at(&c, Context::Tree, '1'), run(Action::TabGoto(0)));
    assert_eq!(at(&c, Context::Tree, 'T'), run(Action::ToggleTabList));
    assert!(c.conflicts.is_empty(), "{:?}", c.conflicts);
}

#[test]
fn a_context_table_overrides_its_layers_and_the_rest_keep_them() {
    let cfg = cfg_of(&[("text", &[("g", "refresh"), ("j", "refresh")])]);
    let m = KeyMap::from_config(KeyScheme::Vim, &cfg);
    assert_eq!(at(&m, Context::Text, 'g'), run(Action::Refresh));
    assert_eq!(at(&m, Context::Text, 'j'), run(Action::Refresh));
    // The document and the visual selection keep the layer's keys.
    assert_eq!(
        at(&m, Context::Document, 'g'),
        run(Action::Navigate(Motion::Top))
    );
    assert_eq!(
        at(&m, Context::TextVisual, 'j'),
        run(Action::Navigate(Motion::Down))
    );
}

// --- config names ---------------------------------------------------------------------------

#[test]
fn a_new_name_reaches_only_its_own_context() {
    let m = KeyMap::from_config(
        KeyScheme::Vim,
        &cfg_of(&[
            ("text", &[("Z", "refresh")]),
            ("document", &[("X", "refresh")]),
        ]),
    );
    assert_eq!(at(&m, Context::Text, 'Z'), run(Action::Refresh));
    assert_eq!(at(&m, Context::Document, 'Z'), Resolution::Unbound);
    assert_eq!(at(&m, Context::Document, 'X'), run(Action::Refresh));
    assert_eq!(at(&m, Context::Text, 'X'), Resolution::Unbound);
    assert!(m.warnings.is_empty());
    assert!(m.renames.is_empty(), "{:?}", m.renames);
}

#[test]
fn an_old_name_reaches_every_context_it_now_covers() {
    let m = KeyMap::from_config(
        KeyScheme::Vim,
        &cfg_of(&[
            ("preview_text", &[("Z", "refresh")]),
            ("preview_text_visual", &[("Z", "tab_new")]),
            ("preview_image", &[("Z", "toggle_info")]),
            ("preview_table", &[("Z", "toggle_help")]),
        ]),
    );
    assert_eq!(at(&m, Context::Text, 'Z'), run(Action::Refresh));
    assert_eq!(at(&m, Context::Document, 'Z'), run(Action::Refresh));
    assert_eq!(at(&m, Context::TextVisual, 'Z'), run(Action::TabNew));
    assert_eq!(at(&m, Context::Image, 'Z'), run(Action::ToggleInfo));
    assert_eq!(at(&m, Context::Table, 'Z'), run(Action::ToggleHelp));
    // The tree is not touched.
    assert_eq!(at(&m, Context::Tree, 'Z'), Resolution::Unbound);
    assert!(m.warnings.is_empty(), "{:?}", m.warnings);
}

#[cfg(feature = "git")]
#[test]
fn an_old_git_name_reaches_git_and_jj() {
    let names = [
        ("git_changes", Screen::Changes),
        ("git_log", Screen::Log),
        ("git_graph", Screen::Graph),
        ("git_graph_picker", Screen::GraphPicker),
        ("git_branches", Screen::Branches),
        ("git_worktrees", Screen::Worktrees),
        ("git_detail", Screen::Detail),
    ];
    for (old, screen) in names {
        let m = KeyMap::from_config(KeyScheme::Vim, &cfg_of(&[(old, &[("Z", "refresh")])]));
        for b in Backend::ALL {
            assert_eq!(
                at(&m, Context::Screen(b, screen), 'Z'),
                run(Action::Refresh),
                "{old} -> {b:?}"
            );
        }
        // No other screen.
        for s in Screen::ALL {
            if s != screen {
                assert_eq!(
                    at(&m, Context::Screen(Backend::Git, s), 'Z'),
                    Resolution::Unbound,
                    "{old} leaked into {s:?}"
                );
            }
        }
        assert!(m.warnings.is_empty(), "{old}: {:?}", m.warnings);
    }
    let m = KeyMap::from_config(
        KeyScheme::Vim,
        &cfg_of(&[("preview_git_diff", &[("Z", "refresh")])]),
    );
    assert_eq!(at(&m, Context::Diff, 'Z'), run(Action::Refresh));
}

#[cfg(feature = "git")]
#[test]
fn the_backends_can_be_configured_apart() {
    let m = KeyMap::from_config(
        KeyScheme::Vim,
        &cfg_of(&[("jj.changes", &[("s", "refresh")])]),
    );
    assert_eq!(
        at(&m, Context::Screen(Backend::Jj, Screen::Changes), 's'),
        run(Action::Refresh)
    );
    assert_eq!(
        at(&m, Context::Screen(Backend::Git, Screen::Changes), 's'),
        run(Action::GitStage)
    );
}

#[test]
fn when_both_names_are_written_the_new_one_wins() {
    // The old name sets Z for text and document; the new name overrides it for text only.
    let m = KeyMap::from_config(
        KeyScheme::Vim,
        &cfg_of(&[
            ("preview_text", &[("Z", "refresh"), ("X", "toggle_info")]),
            ("text", &[("Z", "tab_new")]),
        ]),
    );
    assert_eq!(at(&m, Context::Text, 'Z'), run(Action::TabNew));
    assert_eq!(at(&m, Context::Document, 'Z'), run(Action::Refresh));
    // What the new table does not mention still comes from the old one.
    assert_eq!(at(&m, Context::Text, 'X'), run(Action::ToggleInfo));
    // The same holds when the new name is written first in the file (maps have no order).
    let mut cfg = KeysFileConfig::default();
    let mut new_t = HashMap::new();
    new_t.insert("Z".to_string(), "tab_new".to_string());
    cfg.surfaces.insert("text".into(), new_t);
    let mut old_t = HashMap::new();
    old_t.insert("Z".to_string(), "refresh".to_string());
    cfg.surfaces.insert("preview_text".into(), old_t);
    let m = KeyMap::from_config(KeyScheme::Vim, &cfg);
    assert_eq!(at(&m, Context::Text, 'Z'), run(Action::TabNew));
}

#[cfg(feature = "git")]
#[test]
fn a_new_git_name_beats_the_old_one_for_its_backend_only() {
    let m = KeyMap::from_config(
        KeyScheme::Vim,
        &cfg_of(&[
            ("git_log", &[("Z", "refresh")]),
            ("jj.log", &[("Z", "tab_new")]),
        ]),
    );
    assert_eq!(
        at(&m, Context::Screen(Backend::Git, Screen::Log), 'Z'),
        run(Action::Refresh)
    );
    assert_eq!(
        at(&m, Context::Screen(Backend::Jj, Screen::Log), 'Z'),
        run(Action::TabNew)
    );
}

#[test]
fn old_names_are_noted_with_what_to_write_instead() {
    let m = KeyMap::from_config(
        KeyScheme::Vim,
        &cfg_of(&[
            ("preview_text", &[("Z", "refresh")]),
            ("tree", &[("Z", "refresh")]),
            ("preview_image", &[("Z", "refresh")]),
            ("preview_table", &[]),
        ]),
    );
    // Sorted by the old name; the names that did not change are not noted.
    assert_eq!(
        m.renames,
        vec![
            (
                "[keys.preview_image]".to_string(),
                "[keys.image]".to_string()
            ),
            (
                "[keys.preview_table]".to_string(),
                "[keys.table]".to_string()
            ),
            (
                "[keys.preview_text]".to_string(),
                "[keys.text] + [keys.document]".to_string()
            ),
        ]
    );
    #[cfg(feature = "git")]
    {
        let m = KeyMap::from_config(
            KeyScheme::Vim,
            &cfg_of(&[("git_changes", &[("Z", "refresh")])]),
        );
        assert_eq!(
            m.renames,
            vec![(
                "[keys.git_changes]".to_string(),
                "[keys.\"git.changes\"] + [keys.\"jj.changes\"]".to_string()
            )]
        );
        let m = KeyMap::from_config(
            KeyScheme::Vim,
            &cfg_of(&[("preview_git_diff", &[("Z", "refresh")])]),
        );
        assert_eq!(
            m.renames,
            vec![(
                "[keys.preview_git_diff]".to_string(),
                "[keys.diff]".to_string()
            )]
        );
    }
    // Defaults and the unchanged names note nothing.
    assert!(KeyMap::defaults(KeyScheme::Vim).renames.is_empty());
    let m = KeyMap::from_config(
        KeyScheme::Vim,
        &cfg_of(&[("tree_visual", &[("Z", "refresh")])]),
    );
    assert!(m.renames.is_empty());
}

#[test]
fn a_table_that_covers_several_contexts_warns_once() {
    // The same bad entry under the old `preview_text` (two contexts) warns once, as it did when
    // that was one surface.
    let m = KeyMap::from_config(
        KeyScheme::Vim,
        &cfg_of(&[("preview_text", &[("enter", "quit"), ("zz", "refresh")])]),
    );
    assert_eq!(m.warnings.len(), 2, "{:?}", m.warnings);
}

#[test]
fn a_conflict_in_a_table_that_covers_several_contexts_counts_once() {
    let m = KeyMap::from_config(
        KeyScheme::Vim,
        &cfg_of(&[("preview_text", &[("t", "refresh")])]),
    );
    assert_eq!(m.conflicts.len(), 1, "{:?}", m.conflicts);
    // ...and says which table it was written in.
    assert!(
        m.conflicts[0].dropped.contains("[keys."),
        "{}",
        m.conflicts[0].dropped
    );
    // Written under both new names, it is two separate settings and the first context's
    // conflict is the one listed for the surface.
    let m = KeyMap::from_config(
        KeyScheme::Vim,
        &cfg_of(&[
            ("text", &[("t", "refresh")]),
            ("document", &[("t", "refresh")]),
        ]),
    );
    assert_eq!(m.conflicts.len(), 1);
    // Both were refused, though.
    assert_eq!(at(&m, Context::Text, 't'), run(Action::TabNewAfter));
    assert_eq!(at(&m, Context::Document, 't'), run(Action::TabNewAfter));
}

#[test]
fn an_unknown_context_name_warns_and_the_old_git_names_are_unknown_without_git() {
    let m = KeyMap::from_config(
        KeyScheme::Vim,
        &cfg_of(&[("nonsense", &[("Z", "refresh")])]),
    );
    assert!(m
        .warnings
        .iter()
        .any(|w| w.contains("unknown key surface: nonsense")));
    // `git.changes` is a context only in a build with git.
    let m = KeyMap::from_config(
        KeyScheme::Vim,
        &cfg_of(&[("git.changes", &[("Z", "refresh")])]),
    );
    assert_eq!(
        m.warnings.is_empty(),
        cfg!(feature = "git"),
        "{:?}",
        m.warnings
    );
}

#[test]
fn global_and_the_unchanged_names_are_read_as_before() {
    let m = KeyMap::from_config(
        KeyScheme::Vim,
        &cfg_of(&[
            ("global", &[("W", "tab_close")]),
            ("tree_visual", &[("Z", "refresh")]),
            ("table_cell", &[("Z", "refresh")]),
            ("sort", &[("Z", "refresh")]),
            ("bookmarks", &[("Z", "refresh")]),
            ("tabs", &[("Z", "refresh")]),
            ("outline", &[("Z", "refresh")]),
            ("info", &[("Z", "refresh")]),
            ("help", &[("Z", "refresh")]),
        ]),
    );
    // `[keys.sort]` is read (it re-letters the sort overlay and the tree's `,` menu) and says so.
    assert!(m.warnings.is_empty());
    assert_eq!(m.renames.len(), 1, "{:?}", m.renames);
    assert_eq!(m.renames[0].0, "[keys.sort]");
    assert_eq!(at(&m, Context::Tree, 'W'), run(Action::TabClose));
    for ctx in [
        Context::TreeVisual,
        Context::TableCell,
        Context::Sort,
        Context::Bookmarks,
        Context::Tabs,
        Context::Outline,
        Context::Info,
        Context::Help,
    ] {
        assert_eq!(at(&m, ctx, 'Z'), run(Action::Refresh), "{ctx:?}");
    }
}

#[test]
fn the_conflict_flash_says_where_and_what() {
    let m = KeyMap::from_config(KeyScheme::Vim, &cfg_of(&[("tree", &[("t", "refresh")])]));
    let c = &m.conflicts[0];
    assert_eq!(c.key, KeyPress::ch('t'));
    assert_eq!(c.kept, "tab_new_after");
    assert_eq!(c.dropped, "refresh in [keys.tree]");
}
