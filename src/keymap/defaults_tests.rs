//! The default keys, pinned (plugin stage 2 / B3a). The snapshot lists, for every context, every
//! key that does something there and what it does, written as literals in `defaults_snapshot`: a
//! change to a default key fails here and has to be made on purpose, in the snapshot too. The
//! rest checks the rules the design puts on the table (`docs/FEATURE-PLUGIN-ARCHITECTURE.md`
//! §10-3): layer 1 means the same everywhere, the keys a plugin may take are free, and the keys
//! that cannot be told apart on many terminals are not defaults. The merge of `[keys]` is checked
//! against a small model of its rules.

use super::defaults_snapshot::{LEADERS, LESS_DIFF, VIM};
use super::*;

/// Every `Surface`. `surface_ordinal` is exhaustive, so a new variant fails to compile until it is
/// added there, and `the_surface_list_is_complete` fails until it is added here.
fn all_surfaces() -> Vec<Surface> {
    #[allow(unused_mut)]
    let mut v = vec![
        Surface::DialogInput,
        Surface::Filter,
        Surface::Search,
        Surface::Mark,
        Surface::DialogConfirmDelete,
        Surface::DialogConfirmDrop,
        Surface::DialogRenamePreview,
        Surface::DialogConfirmQuit,
        Surface::DialogConfirmBookmark,
        Surface::Help,
        Surface::Bookmarks,
        Surface::Tabs,
        Surface::Outline,
        Surface::Info,
        Surface::TableCell,
        Surface::Visual,
        Surface::Tree,
        Surface::PreviewText,
        Surface::PreviewTextVisual,
        Surface::PreviewImage,
        Surface::PreviewTable,
        Surface::PreviewCanvas,
        Surface::PluginInput,
        Surface::ListFilter,
    ];
    #[cfg(feature = "git")]
    v.extend([
        Surface::BranchFilter,
        Surface::WorktreeFilter,
        Surface::GitDetail,
        Surface::GitLog,
        Surface::GitGraph,
        Surface::GitGraphPicker,
        Surface::GitBranches,
        Surface::GitChanges,
        Surface::GitWorktrees,
        Surface::PreviewGitDiff,
    ]);
    v
}

fn surface_ordinal(s: Surface) -> usize {
    match s {
        Surface::DialogInput => 0,
        Surface::Filter => 1,
        Surface::Search => 2,
        Surface::Mark => 3,
        Surface::DialogConfirmDelete => 4,
        Surface::DialogConfirmDrop => 5,
        Surface::DialogRenamePreview => 6,
        Surface::DialogConfirmQuit => 7,
        Surface::DialogConfirmBookmark => 8,
        Surface::Help => 9,
        Surface::Bookmarks => 10,
        Surface::Tabs => 11,
        Surface::Outline => 12,
        Surface::Info => 13,
        Surface::TableCell => 14,
        Surface::Visual => 15,
        Surface::Tree => 16,
        Surface::PreviewText => 17,
        Surface::PreviewTextVisual => 18,
        Surface::PreviewImage => 19,
        Surface::PreviewTable => 20,
        Surface::ListFilter => 21,
        Surface::PreviewCanvas => 22,
        Surface::PluginInput => 23,
        #[cfg(feature = "git")]
        Surface::BranchFilter => 24,
        #[cfg(feature = "git")]
        Surface::WorktreeFilter => 25,
        #[cfg(feature = "git")]
        Surface::GitDetail => 26,
        #[cfg(feature = "git")]
        Surface::GitLog => 27,
        #[cfg(feature = "git")]
        Surface::GitGraph => 28,
        #[cfg(feature = "git")]
        Surface::GitGraphPicker => 29,
        #[cfg(feature = "git")]
        Surface::GitBranches => 30,
        #[cfg(feature = "git")]
        Surface::GitChanges => 31,
        #[cfg(feature = "git")]
        Surface::GitWorktrees => 32,
        #[cfg(feature = "git")]
        Surface::PreviewGitDiff => 33,
    }
}

#[test]
fn the_surface_list_is_complete() {
    let all = all_surfaces();
    let mut ords: Vec<usize> = all.iter().map(|&s| surface_ordinal(s)).collect();
    ords.sort_unstable();
    let expected: Vec<usize> = if cfg!(feature = "git") {
        (0..34).collect()
    } else {
        (0..24).collect()
    };
    assert_eq!(ords, expected, "all_surfaces() misses or repeats a Surface");
}

/// Every state a key can arrive in: (surface, text preview is a decorated document, git screens
/// are open for jj). The two flags only matter to the surfaces they refine; the rest are visited
/// once per flag value anyway, which also checks that the flags do not leak.
fn all_states() -> Vec<(Surface, bool, bool)> {
    let mut v = Vec::new();
    for s in all_surfaces() {
        for doc in [false, true] {
            for jj in [false, true] {
                // Visiting the unrefined surfaces four times would only repeat work.
                let refined_by_doc = s == Surface::PreviewText;
                let refined_by_jj = surface_has_backend(s);
                if (doc && !refined_by_doc) || (jj && !refined_by_jj) {
                    continue;
                }
                v.push((s, doc, jj));
            }
        }
    }
    v
}

fn surface_has_backend(s: Surface) -> bool {
    #[cfg(feature = "git")]
    {
        matches!(
            s,
            Surface::GitDetail
                | Surface::GitLog
                | Surface::GitGraph
                | Surface::GitGraphPicker
                | Surface::GitBranches
                | Surface::GitChanges
                | Surface::GitWorktrees
        )
    }
    #[cfg(not(feature = "git"))]
    {
        let _ = s;
        false
    }
}

/// Every key the terminal can send: printable ASCII and its Ctrl version, the named keys, F1-F12.
fn all_keys() -> Vec<KeyPress> {
    let mut v = Vec::new();
    for b in 0x20u8..=0x7e {
        v.push(KeyPress::ch(char::from(b)));
        v.push(KeyPress::ctrl_ch(char::from(b)));
    }
    for code in [
        KeyCode::Esc,
        KeyCode::Enter,
        KeyCode::Tab,
        KeyCode::BackTab,
        KeyCode::Up,
        KeyCode::Down,
        KeyCode::Left,
        KeyCode::Right,
        KeyCode::Home,
        KeyCode::End,
        KeyCode::PageUp,
        KeyCode::PageDown,
        KeyCode::Backspace,
        KeyCode::Delete,
    ] {
        v.push(KeyPress::key(code));
    }
    for n in 1..=12 {
        v.push(KeyPress::key(KeyCode::F(n)));
    }
    v
}

/// Every context is reached from some surface state, and every keymap-driven surface state
/// reaches one (so no context is dead and no surface falls through).
#[test]
fn the_contexts_and_the_surface_states_cover_each_other() {
    let mut reached: Vec<Context> = Vec::new();
    for (s, doc, jj) in all_states() {
        match Context::of_surface(s, doc, jj) {
            Some(c) => {
                assert!(
                    s.allows_tabs(),
                    "{s:?} has a context but is a fixed surface"
                );
                reached.push(c);
            }
            None => assert!(!s.allows_tabs(), "{s:?} uses the keymap but has no context"),
        }
    }
    reached.sort_by_key(|c| c.name());
    reached.dedup();
    let mut all = Context::all();
    all.sort_by_key(|c| c.name());
    assert_eq!(reached, all);
}

fn all_leaders() -> Vec<LeaderId> {
    #[allow(unused_mut)]
    let mut v = vec![LeaderId::Copy, LeaderId::File, LeaderId::TableCopy];
    #[cfg(feature = "git")]
    v.extend([LeaderId::GitCopy, LeaderId::BranchCopy]);
    v
}

// ---------------------------------------------------------------------------------------------
// The snapshot.
// ---------------------------------------------------------------------------------------------

/// What a key does in a context, written as the snapshot does (`-` = nothing).
fn describe(m: &KeyMap, ctx: Option<Context>, kp: KeyPress) -> Option<String> {
    let r = match ctx {
        Some(c) => m.resolve(c, None, kp),
        None => match m.global.get(&kp) {
            Some(b) => binding_to_resolution(b),
            None => Resolution::Unbound,
        },
    };
    match r {
        Resolution::Action(a) => Some(action_name(a)),
        Resolution::EnterLeader(id) => Some(format!("leader:{}", leader_name(id))),
        Resolution::Unbound => None,
    }
}

/// One line per context and key: `context | key | what it does`, contexts by name, keys in the
/// order the help lists them. `None` is `global` alone.
fn dump(m: &KeyMap) -> String {
    let keys = all_keys();
    let mut ctxs: Vec<Option<Context>> = vec![None];
    ctxs.extend(Context::all().into_iter().map(Some));
    ctxs.sort_by_key(|c| c.map(|c| c.name()).unwrap_or_else(|| "global".into()));
    let mut out = String::new();
    for ctx in ctxs {
        let name = ctx.map(|c| c.name()).unwrap_or_else(|| "global".into());
        let mut rows: Vec<(KeyPress, String)> = keys
            .iter()
            .filter_map(|&k| describe(m, ctx, k).map(|d| (k, d)))
            .collect();
        rows.sort_by_key(|(k, _)| display_order(*k));
        for (k, d) in rows {
            out.push_str(&format!("{name} | {} | {d}\n", k.label()));
        }
    }
    out
}

/// The lines of `less` that `vim` does not have (and the other way round), as `+`/`-` lines.
fn diff_lines(vim: &str, less: &str) -> String {
    let a: Vec<&str> = vim.lines().collect();
    let b: Vec<&str> = less.lines().collect();
    let mut out = String::new();
    for l in &a {
        if !b.contains(l) {
            out.push_str(&format!("- {l}\n"));
        }
    }
    for l in &b {
        if !a.contains(l) {
            out.push_str(&format!("+ {l}\n"));
        }
    }
    out
}

fn leader_dump(m: &KeyMap) -> String {
    let mut out = String::new();
    let mut ids = all_leaders();
    ids.sort_by_key(|&i| leader_name(i));
    for id in ids {
        for it in &m.leaders[&id].items {
            out.push_str(&format!(
                "{} | {} | {}\n",
                leader_name(id),
                it.key.label(),
                action_name(it.action)
            ));
        }
    }
    // The menus a context has of its own (the `,` menus), `<menu>@<context>`.
    let mut own: Vec<(&(Context, LeaderId), &LeaderMenu)> = m.ctx_menus.iter().collect();
    own.sort_by_key(|((c, id), _)| (c.name(), leader_name(*id)));
    for ((c, id), menu) in own {
        for it in &menu.items {
            out.push_str(&format!(
                "{}@{} | {} | {}\n",
                leader_name(*id),
                c.name(),
                it.key.label(),
                action_name(it.action)
            ));
        }
    }
    out
}

/// Prints the three snapshots when `KONOMA_PRINT_SNAPSHOTS` is set
/// (`KONOMA_PRINT_SNAPSHOTS=1 cargo test print_the_default_snapshots -- --nocapture`), so a
/// deliberate change can be pasted into `defaults_snapshot`. Without it, the test only checks that
/// the three dumps can be made.
#[test]
fn print_the_default_snapshots() {
    if std::env::var_os("KONOMA_PRINT_SNAPSHOTS").is_none() {
        assert!(!dump(&KeyMap::defaults(KeyScheme::Vim)).is_empty());
        return;
    }
    let vim = dump(&KeyMap::defaults(KeyScheme::Vim));
    let less = dump(&KeyMap::defaults(KeyScheme::Less));
    println!(
        "=== VIM ===\n{vim}=== LESS_DIFF ===\n{}",
        diff_lines(&vim, &less)
    );
    println!(
        "=== LEADERS ===\n{}",
        leader_dump(&KeyMap::defaults(KeyScheme::Vim))
    );
}

fn only_in_this_build(snapshot: &str) -> String {
    // Lines of the version-control contexts are in the snapshot of a build with `git`; a build
    // without it has none of them.
    snapshot
        .lines()
        .filter(|l| {
            // (a line of the less difference starts with `- ` or `+ `)
            let ctx = l.split(" | ").next().unwrap_or("");
            let ctx = ctx.trim_start_matches(['-', '+', ' ']);
            cfg!(feature = "git")
                || !(ctx == "diff"
                    || ctx.starts_with("git.")
                    || ctx.starts_with("jj.")
                    // `!` launches the project's tool, which is a version-control operation.
                    || l.ends_with("| git_launch_tool"))
        })
        .map(|l| format!("{l}\n"))
        .collect()
}

fn same(what: &str, got: &str, want: &str) {
    if got != want {
        let g: Vec<&str> = got.lines().collect();
        let w: Vec<&str> = want.lines().collect();
        let extra: Vec<&&str> = g.iter().filter(|l| !w.contains(l)).take(15).collect();
        let missing: Vec<&&str> = w.iter().filter(|l| !g.contains(l)).take(15).collect();
        panic!("{what} differs from the snapshot.\nnot in the snapshot (first 15):\n{extra:#?}\nonly in the snapshot (first 15):\n{missing:#?}");
    }
}

#[test]
fn the_default_keys_are_the_snapshot_vim() {
    same(
        "the default keys",
        &dump(&KeyMap::defaults(KeyScheme::Vim)),
        &only_in_this_build(VIM),
    );
}

#[test]
fn the_less_scheme_differs_from_vim_only_in_the_paging_rows() {
    let vim = dump(&KeyMap::defaults(KeyScheme::Vim));
    let less = dump(&KeyMap::defaults(KeyScheme::Less));
    same(
        "the less scheme's difference",
        &diff_lines(&vim, &less),
        &only_in_this_build(LESS_DIFF),
    );
}

#[test]
fn the_default_menus_are_the_snapshot() {
    let got = leader_dump(&KeyMap::defaults(KeyScheme::Vim));
    let want: String = LEADERS
        .lines()
        .filter(|l| {
            cfg!(feature = "git")
                || !(l.starts_with("git_copy")
                    || l.starts_with("branch_copy")
                    || l.starts_with("options@diff"))
        })
        .map(|l| format!("{l}\n"))
        .collect();
    same("the default menus", &got, &want);
}

// ---------------------------------------------------------------------------------------------
// The rules the design puts on the table.
// ---------------------------------------------------------------------------------------------

/// Keys that cannot be told apart from others on many terminals, or that the terminal takes
/// (design 10-3-2, rule 8). None may be a default anywhere.
#[test]
fn the_keys_that_terminals_cannot_tell_apart_are_not_defaults() {
    let forbidden = ['j', 'm', 'i', 'h', '[', 's', 'q'];
    for scheme in [KeyScheme::Vim, KeyScheme::Less] {
        let m = KeyMap::defaults(scheme);
        for c in forbidden {
            let kp = KeyPress::ctrl_ch(c);
            assert!(
                !m.global.contains_key(&kp),
                "{scheme:?}: global binds Ctrl-{c}"
            );
            assert!(
                !m.basic.contains_key(&kp),
                "{scheme:?}: layer 1 binds Ctrl-{c}"
            );
            for (vt, t) in &m.types {
                assert!(!t.contains_key(&kp), "{scheme:?}: {vt:?} binds Ctrl-{c}");
            }
            for (ctx, t) in &m.contexts {
                assert!(
                    !t.contains_key(&kp),
                    "{scheme:?}: {} binds Ctrl-{c}",
                    ctx.name()
                );
            }
        }
        // And no modifier + Enter (only Ctrl-letters exist in the key model, so this is Enter
        // itself being a fixed key, never a binding).
        for (ctx, t) in &m.contexts {
            assert!(
                !t.keys().any(|k| k.code == KeyCode::Enter),
                "{}: Enter is a fixed key",
                ctx.name()
            );
        }
    }
    // The plugin crate's own list agrees with this one.
    let theirs: Vec<char> = konoma_plugin::keys::forbidden_defaults()
        .iter()
        .filter_map(|k| match k.code {
            konoma_plugin::keys::KeyName::Char(c) if k.ctrl => Some(c),
            _ => None,
        })
        .collect();
    assert_eq!(theirs, forbidden.to_vec());
}

/// Layer 4: the keys a context binds that no layer reserves are the plugin's (the version-control
/// screens' own: `s u S U x c d b w`...). They must not be keys the plugin crate reserves for
/// layers 0 to 3 in that kind of context, apart from the ones that *are* the layers' meaning
/// there (listed here with the meaning).
#[cfg(feature = "git")]
#[test]
fn the_git_screens_use_a_layer_four_key_only_where_no_layer_reserves_it() {
    // (key, the layer-1..3 meaning a list screen may give it).
    let layer_meaning: &[(&str, &str)] = &[
        ("h", "back (q)"),
        ("l", "open (Enter)"),
        ("q", "back"),
        ("/", "filter"),
        ("y", "copy menu"),
        ("Ctrl-t", "open in a new tab"),
        ("!", "launch the project tool"),
        ("r", "refresh (layer 1)"),
        ("e", "edit the row's file (layer 3)"),
        ("o", "open the row's file in another app (layer 3)"),
        ("J", "reorder in the branch panel"),
        ("K", "reorder in the branch panel"),
    ];
    use konoma_plugin::keys::{reserved, KeySpec};
    let m = KeyMap::defaults(KeyScheme::Vim);
    let spec = |kp: KeyPress| -> KeySpec {
        match kp.code {
            KeyCode::Char(c) if kp.ctrl => KeySpec::ctrl(c),
            KeyCode::Char(c) => KeySpec::ch(c),
            other => panic!("unexpected key {other:?}"),
        }
    };
    for backend in Backend::ALL {
        for screen in Screen::ALL {
            // The commit detail is a text view, not a list: its keys are the text layer's.
            if screen == Screen::Detail {
                continue;
            }
            let ctx = Context::Screen(backend, screen);
            let res: Vec<KeySpec> = reserved("list").into_iter().map(|(k, _)| k).collect();
            for kp in m.contexts[&ctx].keys().copied() {
                let label = kp.label();
                if layer_meaning.iter().any(|(k, _)| *k == label) {
                    continue;
                }
                assert!(
                    !res.contains(&spec(kp)),
                    "{} binds `{label}`, which layers 0-3 reserve in a list",
                    ctx.name()
                );
            }
        }
    }
}

/// Layer 1 means the same in every context: for each layer-1 key, the operations it runs across
/// the contexts that bind it. Anything different is listed here with the reason, so a new
/// difference cannot slip in. (`Navigate` motions of one kind count as the same: "down" is
/// "down" whatever the view moves.)
#[test]
fn layer_one_keys_mean_the_same_in_every_context() {
    let m = KeyMap::defaults(KeyScheme::Vim);
    // Contexts whose letters are not a cursor's keys: the info popup is a one-shot picker with its
    // own letters.
    let pickers = [Context::Info];
    // key -> the operations that mean the same thing for it. Anything else is a violation.
    let same_meaning = |kp: KeyPress| -> Vec<String> {
        let nav = |mo: Motion| action_name(Action::Navigate(mo));
        let l = kp.label().replace("Ctrl-", "ctrl-");
        match l.as_str() {
            "j" => vec![nav(Motion::Down)],
            "k" => vec![nav(Motion::Up)],
            "g" => vec![nav(Motion::Top)],
            "G" => vec![nav(Motion::Bottom)],
            "0" => vec![nav(Motion::LineHome)],
            "$" => vec![nav(Motion::LineEnd)],
            "ctrl-d" => vec![nav(Motion::HalfDown)],
            "ctrl-u" => vec![nav(Motion::HalfUp)],
            "ctrl-f" => vec![nav(Motion::PageDown)],
            "ctrl-b" => vec![nav(Motion::PageUp)],
            // h / l: left and right, or leave and open in the tree and the lists.
            "h" => vec![
                nav(Motion::Left),
                action_name(Action::TreeLeave),
                "git_close".to_string(),
                "worktree_close".to_string(),
                // The outline / page list and the tab list: back (`q`).
                action_name(Action::ToggleOutline),
                action_name(Action::ToggleTabList),
            ],
            "l" => vec![
                nav(Motion::Right),
                action_name(Action::TreeDescend),
                "git_open_selected_diff".to_string(),
                "git_open_detail".to_string(),
                "branch_checkout".to_string(),
                "worktree_goto".to_string(),
                "git_graph_picker_apply".to_string(),
                // The outline / page list and the tab list: open (`Enter`).
                action_name(Action::OutlineJump),
                action_name(Action::TabListActivate),
            ],
            // q: go back / close, whatever there is to close.
            "q" => vec![
                action_name(Action::CloseTabOrQuit),
                action_name(Action::PreviewBack),
                action_name(Action::Cancel),
                action_name(Action::PreviewExitVisual),
                "git_close".to_string(),
                "worktree_close".to_string(),
                action_name(Action::BookmarkClose),
                action_name(Action::ToggleTabList),
                action_name(Action::ToggleOutline),
                action_name(Action::InfoClose),
                action_name(Action::ToggleTableCell),
                action_name(Action::ToggleHelp),
            ],
            // / : start a search or a filter.
            "/" => vec![
                action_name(Action::FilterStart),
                action_name(Action::SearchStart),
                "branch_filter_start".to_string(),
                "worktree_filter_start".to_string(),
                "list_filter_start".to_string(),
            ],
            // n / N : the next / previous match or change.
            "n" => vec![
                action_name(Action::SearchNext),
                action_name(Action::JumpNextChange),
                "diff_next_change".to_string(),
            ],
            "N" => vec![
                action_name(Action::SearchPrev),
                action_name(Action::JumpPrevChange),
                "diff_prev_change".to_string(),
            ],
            "v" => vec![
                action_name(Action::EnterVisual),
                action_name(Action::VisualCommit),
                action_name(Action::PreviewEnterVisual),
                action_name(Action::PreviewExitVisual),
            ],
            "V" => vec![
                action_name(Action::ToggleSelect),
                action_name(Action::PreviewEnterVisualLine),
                action_name(Action::PreviewExitVisual),
            ],
            "r" => vec![action_name(Action::Refresh)],
            "m" => vec![action_name(Action::MarkSet)],
            "'" => vec![
                action_name(Action::MarkJump),
                action_name(Action::BookmarkClose),
            ],
            // Space: the menu of operations (the tree's is the file menu). `,`: the options.
            " " | "Space" => vec!["leader".to_string()],
            "," => vec!["leader".to_string()],
            "ctrl-t" => vec![
                action_name(Action::OpenInNewTab),
                action_name(Action::OpenLinkNewTab),
                "worktree_goto_new_tab".to_string(),
            ],
            "ctrl-n" => vec![
                action_name(Action::PreviewFileNext),
                action_name(Action::JumpNextChange),
            ],
            "ctrl-p" => vec![
                action_name(Action::PreviewFilePrev),
                action_name(Action::JumpPrevChange),
            ],
            _ => Vec::new(),
        }
    };
    let layer_one: Vec<KeyPress> = [
        "j", "k", "g", "G", "0", "$", "h", "l", "q", "/", "n", "N", "v", "V", "r", "m", "'",
        "space", ",",
    ]
    .iter()
    .map(|s| KeyPress::parse(s).unwrap())
    .chain(
        [
            "ctrl-d", "ctrl-u", "ctrl-f", "ctrl-b", "ctrl-t", "ctrl-n", "ctrl-p",
        ]
        .iter()
        .map(|s| KeyPress::parse(s).unwrap()),
    )
    .collect();
    let mut bad: Vec<String> = Vec::new();
    for ctx in Context::all() {
        if pickers.contains(&ctx) {
            continue;
        }
        for &kp in &layer_one {
            let got = match m.resolve(ctx, None, kp) {
                Resolution::Action(a) => action_name(a),
                Resolution::EnterLeader(_) => "leader".to_string(),
                Resolution::Unbound => continue,
            };
            if !same_meaning(kp).contains(&got) {
                bad.push(format!("{} | {} | {got}", ctx.name(), kp.label()));
            }
        }
    }
    assert!(
        bad.is_empty(),
        "layer-1 keys with another meaning:\n{}",
        bad.join("\n")
    );
}

// ---------------------------------------------------------------------------------------------
// The merge of `[keys]`, against a small model of its rules.
// ---------------------------------------------------------------------------------------------

/// One-entry tables that probe the merge rules, each written under every name.
const PROBES: &[(&str, &str)] = &[
    ("j", "noop"),    // unbind a key the layers give
    ("k", "refresh"), // override a layer-1 key
    ("g", "refresh"), // override a key that is layer 2 in some, own in others
    ("G", "noop"),
    ("h", "noop"),
    ("l", "tab_new"), // bind a global action
    ("0", "noop"),
    ("$", "refresh"),
    ("q", "noop"),
    ("Z", "refresh"), // a new key
    ("t", "refresh"), // steal a global key (conflict)
    ("T", "noop"),    // unbind a global key in a context
    ("Q", "refresh"),
    ("1", "refresh"), // steal a tab-goto digit
    ("?", "refresh"),
    ("y", "quit"), // steal a leader prefix (conflict where it is one)
    ("space", "quit"),
    ("pagedown", "noop"),
    ("ctrl-f", "refresh"),
    ("f", "navigate:page_down"),
    ("ctrl-d", "noop"),
    ("d", "noop"),
    ("R", "noop"),
    ("s", "navigate:down"),
    ("n", "noop"),
    ("v", "refresh"),
    ("c", "git_stage"),
    ("O", "noop"),
    ("p", "open_git_view"),
];

/// Probes that must be refused with exactly one warning and change nothing.
const REFUSED: &[(&str, &str)] = &[
    ("enter", "quit"),         // a fixed key
    ("zz", "refresh"),         // not a key
    ("x", "no_such_action"),   // not an action
    ("a", "tab_goto"),         // a fixed action
    ("g s", "open_sort_menu"), // an anonymous prefix
];

/// Every `[keys.<name>]` name the keymap reads: the contexts' own, the old surface names, and a
/// name that is neither.
fn names() -> Vec<String> {
    let mut v: Vec<String> = vec!["global".into(), "nonsense".into()];
    v.extend(Context::all().iter().map(|c| c.name()));
    v.extend(
        [
            "preview_text",
            "preview_text_visual",
            "preview_image",
            "preview_table",
        ]
        .map(String::from),
    );
    #[cfg(feature = "git")]
    v.extend(
        [
            "preview_git_diff",
            "git_changes",
            "git_log",
            "git_graph",
            "git_graph_picker",
            "git_branches",
            "git_worktrees",
            "git_detail",
        ]
        .map(String::from),
    );
    v
}

fn cfg_from(tables: &[(&str, &[(&str, &str)])]) -> KeysFileConfig {
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

/// The contexts a table name stands for (`None` = `global`; an empty list = unknown name).
fn covered(name: &str) -> Option<Vec<Context>> {
    if name == "global" {
        return None;
    }
    if let Some(c) = Context::from_name(name) {
        return Some(vec![c]);
    }
    Some(old_surface_contexts(name).unwrap_or_default())
}

/// What the rules say `kp` resolves to in `ctx` after the one entry `kp = action` is merged into a
/// table that covers `ctx`. `def` is the default keymap.
fn expected_after(def: &KeyMap, ctx: Context, kp: KeyPress, action: &str) -> Resolution {
    let default = def.resolve(ctx, None, kp);
    let global = def
        .global
        .get(&kp)
        .map(binding_to_resolution)
        .unwrap_or(Resolution::Unbound);
    if action == "noop" || action == "disabled" {
        // Unbinding hides what the context and its layers give; `global` still applies.
        return global;
    }
    let a = action_from_str(action).expect("probe actions exist");
    let wanted = Resolution::Action(a);
    // A key that is a leader's prefix by default stays the leader (a conflict is dropped).
    if matches!(def.local(ctx, kp), Some(Binding::Leader(_))) {
        return default;
    }
    // A context may not take a global key for another operation, unless its default already did.
    if global != Resolution::Unbound && global != wanted && default != wanted {
        return default;
    }
    wanted
}

#[test]
fn a_merged_entry_follows_the_rules_for_every_name_and_probe() {
    let mut cases = 0;
    for scheme in [KeyScheme::Vim, KeyScheme::Less] {
        let def = KeyMap::defaults(scheme);
        for name in names() {
            for &(k, a) in PROBES {
                // An operation this build does not have (`git_stage` without git) is not probed.
                if a != "noop" && action_from_str(a).is_none() {
                    continue;
                }
                let kp = match KeyChord::parse(k).unwrap() {
                    KeyChord::Single(kp) => kp,
                    KeyChord::Chord(..) => unreachable!("probes are single keys"),
                };
                let m = KeyMap::from_config(scheme, &cfg_from(&[(name.as_str(), &[(k, a)])]));
                let what = format!("{scheme:?} [keys.{name}] {k} = {a}");
                match covered(&name) {
                    // An unknown name warns and changes nothing.
                    Some(cs) if cs.is_empty() => {
                        assert_eq!(m.warnings.len(), 1, "{what}: {:?}", m.warnings);
                        for ctx in Context::all() {
                            assert_eq!(
                                m.resolve(ctx, None, kp),
                                def.resolve(ctx, None, kp),
                                "{what}"
                            );
                        }
                    }
                    Some(cs) => {
                        for ctx in Context::all() {
                            let want = if cs.contains(&ctx) {
                                expected_after(&def, ctx, kp, a)
                            } else {
                                def.resolve(ctx, None, kp)
                            };
                            assert_eq!(m.resolve(ctx, None, kp), want, "{what} in {}", ctx.name());
                        }
                    }
                    // `global`: every context that does not own the key follows the new binding.
                    None => {
                        let new = if a == "noop" {
                            None
                        } else {
                            Some(Resolution::Action(action_from_str(a).unwrap()))
                        };
                        for ctx in Context::all() {
                            if def.local(ctx, kp).is_some() {
                                continue; // the context's own binding wins over global
                            }
                            // A fixed key or a leader prefix may refuse or roll back; a plain
                            // rebind of a key no context owns is what is checked here.
                            if matches!(def.global.get(&kp), Some(Binding::Leader(_))) {
                                continue;
                            }
                            let want = new.unwrap_or(Resolution::Unbound);
                            assert_eq!(m.resolve(ctx, None, kp), want, "{what} in {}", ctx.name());
                        }
                    }
                }
                cases += 1;
            }
        }
    }
    assert!(cases > 1000, "{cases}");
}

#[test]
fn a_refused_entry_warns_once_and_changes_nothing() {
    for scheme in [KeyScheme::Vim, KeyScheme::Less] {
        let def = KeyMap::defaults(scheme);
        for name in names() {
            if covered(&name).is_some_and(|c| c.is_empty()) {
                continue;
            }
            for &(k, a) in REFUSED {
                let m = KeyMap::from_config(scheme, &cfg_from(&[(name.as_str(), &[(k, a)])]));
                assert_eq!(
                    m.warnings.len(),
                    1,
                    "[keys.{name}] {k} = {a}: {:?}",
                    m.warnings
                );
                assert_eq!(
                    dump(&m),
                    dump(&def),
                    "[keys.{name}] {k} = {a} changed a key"
                );
            }
        }
    }
}

/// A menu entry written in a `[keys.<context>]` table reaches the menu that key opens *in that
/// context* and no other: the `y` of the tree, of the text and of a table are three menus.
/// `[keys.global]` keeps reaching the shared menus (and every context's copy of them), as it did
/// when there was one `y` menu for everything.
#[test]
fn a_menu_entry_reaches_the_menu_of_the_context_it_is_written_in() {
    let z = KeyPress::ch('z');
    let copy_full = Resolution::Action(Action::CopyPath(CopyKind::Full));
    let at = |m: &KeyMap, c: Context, id: LeaderId| m.resolve_leader(c, id, z);
    for scheme in [KeyScheme::Vim, KeyScheme::Less] {
        // tree: its own `y` and `Space` menus, nothing else.
        let m = KeyMap::from_config(
            scheme,
            &cfg_from(&[("tree", &[("y z", "copy_full"), ("space z", "file_delete")])]),
        );
        assert!(m.warnings.is_empty(), "{:?}", m.warnings);
        assert_eq!(at(&m, Context::Tree, LeaderId::Copy), copy_full);
        assert_eq!(
            at(&m, Context::Tree, LeaderId::File),
            Resolution::Action(Action::FileDelete)
        );
        for c in [
            Context::Text,
            Context::Document,
            Context::Image,
            Context::TreeVisual,
        ] {
            assert_eq!(
                at(&m, c, LeaderId::Copy),
                Resolution::Unbound,
                "{}",
                c.name()
            );
        }
        assert_eq!(
            at(&m, Context::TreeVisual, LeaderId::File),
            Resolution::Unbound
        );

        // text: only the text's copy menu (a document is a different context).
        let m = KeyMap::from_config(scheme, &cfg_from(&[("text", &[("y z", "copy_full")])]));
        assert!(m.warnings.is_empty(), "{:?}", m.warnings);
        assert_eq!(at(&m, Context::Text, LeaderId::Copy), copy_full);
        for c in [Context::Tree, Context::Document, Context::Image] {
            assert_eq!(
                at(&m, c, LeaderId::Copy),
                Resolution::Unbound,
                "{}",
                c.name()
            );
        }

        // table: the table's own copy menu.
        let m = KeyMap::from_config(scheme, &cfg_from(&[("table", &[("y z", "copy_full")])]));
        assert_eq!(at(&m, Context::Table, LeaderId::TableCopy), copy_full);
        assert_eq!(at(&m, Context::Tree, LeaderId::Copy), Resolution::Unbound);

        // global: the shared menus, as before, and every context that shows them.
        let m = KeyMap::from_config(
            scheme,
            &cfg_from(&[(
                "global",
                &[("y z", "copy_full"), ("space z", "file_delete")],
            )]),
        );
        assert!(m.warnings.is_empty(), "{:?}", m.warnings);
        for c in [
            Context::Tree,
            Context::Text,
            Context::Document,
            Context::Image,
        ] {
            assert_eq!(at(&m, c, LeaderId::Copy), copy_full, "{}", c.name());
        }
        assert_eq!(
            at(&m, Context::Tree, LeaderId::File),
            Resolution::Action(Action::FileDelete)
        );
    }
}

/// The `Space` menu outside the tree lists the context's operations itself, so a chord on it in
/// a context's table says why it is refused; a context with no menu on the key is refused too.
/// The old `[keys.sort]` table re-letters the tree's `,` menu now, and says so at start-up.
#[test]
fn the_old_sort_table_re_letters_the_options_menu() {
    let m = KeyMap::from_config(
        KeyScheme::Vim,
        &cfg_from(&[("sort", &[("z", "sort_size"), ("n", "noop")])]),
    );
    assert!(m.warnings.is_empty(), "{:?}", m.warnings);
    assert_eq!(m.renames.len(), 1);
    let at = |k: char| m.resolve_leader(Context::Tree, LeaderId::Options, KeyPress::ch(k));
    assert_eq!(at('z'), Resolution::Action(Action::SortSet(SortKey::Size)));
    assert_eq!(at('n'), Resolution::Unbound);
    // The other menus are untouched.
    assert_eq!(
        m.resolve_leader(Context::Text, LeaderId::Options, KeyPress::ch('z')),
        Resolution::Unbound
    );
}

#[test]
fn a_menu_entry_for_a_menu_that_cannot_hold_it_is_refused_once() {
    for scheme in [KeyScheme::Vim, KeyScheme::Less] {
        for (name, chord) in [("text", "space z"), ("table", "space z")] {
            let m = KeyMap::from_config(scheme, &cfg_from(&[(name, &[(chord, "refresh")])]));
            assert_eq!(
                m.warnings.len(),
                1,
                "[keys.{name}] {chord}: {:?}",
                m.warnings
            );
        }
        // A context with a `,` menu takes the entry, one without says so.
        let m = KeyMap::from_config(scheme, &cfg_from(&[("text", &[(", z", "refresh")])]));
        assert!(m.warnings.is_empty(), "{:?}", m.warnings);
        assert_eq!(
            m.resolve_leader(Context::Text, LeaderId::Options, KeyPress::ch('z')),
            Resolution::Action(Action::Refresh)
        );
        let m = KeyMap::from_config(scheme, &cfg_from(&[("bookmarks", &[(", z", "refresh")])]));
        assert_eq!(m.warnings.len(), 1, "{:?}", m.warnings);
    }
}

#[test]
fn several_tables_at_once_merge_each_into_its_own_contexts() {
    for scheme in [KeyScheme::Vim, KeyScheme::Less] {
        let cfg = cfg_from(&[
            ("global", &[("W", "tab_close"), ("t", "refresh")]),
            ("tree", &[("j", "noop"), ("space z", "file_delete")]),
            ("preview_text", &[("q", "noop"), ("o", "refresh")]),
            ("preview_image", &[("=", "noop"), ("g", "navigate:top")]),
            ("preview_table", &[("J", "noop"), ("y", "quit")]),
            ("sort", &[("n", "noop")]),
            ("bookmarks", &[("j", "noop"), ("e", "refresh")]),
            ("nonsense", &[("a", "refresh")]),
        ]);
        let m = KeyMap::from_config(scheme, &cfg);
        let at = |c: Context, k: char| m.resolve(c, None, KeyPress::ch(k));
        // global: a new key everywhere; `t` is a global key a context may not take over, but
        // `global` itself may change it.
        assert_eq!(at(Context::Tree, 'W'), Resolution::Action(Action::TabClose));
        assert_eq!(at(Context::Image, 't'), Resolution::Action(Action::Refresh));
        // tree
        assert_eq!(at(Context::Tree, 'j'), Resolution::Unbound);
        assert_eq!(
            at(Context::Tabs, 'j'),
            Resolution::Action(Action::Navigate(Motion::Down))
        );
        // the old name `preview_text` covers text and document
        for c in [Context::Text, Context::Document] {
            assert_eq!(at(c, 'q'), Resolution::Unbound);
            assert_eq!(at(c, 'o'), Resolution::Action(Action::Refresh));
        }
        assert_eq!(
            at(Context::Image, 'q'),
            Resolution::Action(Action::PreviewBack)
        );
        // image / table / sort / bookmarks
        assert_eq!(at(Context::Image, '='), Resolution::Unbound);
        assert_eq!(
            at(Context::Image, 'g'),
            Resolution::Action(Action::Navigate(Motion::Top))
        );
        assert_eq!(at(Context::Table, 'J'), Resolution::Unbound);
        // `[keys.sort] n = noop` takes the letter out of the tree's `,` menu, where the old sort
        // overlay's letters live now.
        assert_eq!(
            m.resolve_leader(Context::Tree, LeaderId::Options, KeyPress::ch('n')),
            Resolution::Unbound
        );
        assert_eq!(
            at(Context::Bookmarks, 'e'),
            Resolution::Action(Action::Refresh)
        );
        assert_eq!(
            m.warnings.len(),
            1,
            "only `nonsense` warns: {:?}",
            m.warnings
        );
        assert_eq!(
            m.resolve_leader(Context::Tree, LeaderId::File, KeyPress::ch('z')),
            Resolution::Action(Action::FileDelete)
        );
    }
}

#[cfg(feature = "git")]
#[test]
fn several_git_tables_at_once_merge_each_into_its_own_contexts() {
    for scheme in [KeyScheme::Vim, KeyScheme::Less] {
        let cfg = cfg_from(&[
            (
                "git_changes",
                &[("h", "noop"), ("G", "navigate:down"), ("l", "noop")],
            ),
            ("git_log", &[("l", "noop"), ("g", "refresh")]),
            ("git_graph", &[("x", "noop"), ("z", "git_graph_clear_base")]),
            ("git_graph_picker", &[("g", "noop"), ("c", "noop")]),
            ("git_branches", &[("c", "noop"), ("G", "noop")]),
            ("git_worktrees", &[("c", "refresh")]),
            ("git_detail", &[("s", "noop"), ("ctrl-f", "noop")]),
            (
                "preview_git_diff",
                &[("x", "noop"), ("s", "noop"), ("J", "noop")],
            ),
        ]);
        let m = KeyMap::from_config(scheme, &cfg);
        assert!(m.warnings.is_empty(), "{:?}", m.warnings);
        for b in Backend::ALL {
            let at = |s: Screen, k: char| m.resolve(Context::Screen(b, s), None, KeyPress::ch(k));
            assert_eq!(at(Screen::Changes, 'h'), Resolution::Unbound);
            assert_eq!(at(Screen::Changes, 'l'), Resolution::Unbound);
            assert_eq!(
                at(Screen::Changes, 'G'),
                Resolution::Action(Action::Navigate(Motion::Down))
            );
            assert_eq!(at(Screen::Log, 'g'), Resolution::Action(Action::Refresh));
            assert_eq!(at(Screen::Graph, 'x'), Resolution::Unbound);
            assert_eq!(
                at(Screen::Graph, 'z'),
                Resolution::Action(Action::GitGraphClearBase)
            );
            assert_eq!(at(Screen::GraphPicker, 'c'), Resolution::Unbound);
            assert_eq!(at(Screen::Branches, 'c'), Resolution::Unbound);
            assert_eq!(
                at(Screen::Worktrees, 'c'),
                Resolution::Action(Action::Refresh)
            );
        }
        assert_eq!(m.renames.len(), 7 + 1);
    }
}

/// The reverse of the check above: a default key that means one of layers 0 to 3 (or the layer 2
/// of its display type) must be in the table the plugin crate reserves for that kind of context,
/// or a plugin could take a key the host already uses. The contexts are mapped to the plugin
/// crate's names (tree, text, document, table, image, diff, list).
#[test]
fn every_default_key_with_a_layer_meaning_is_reserved_for_plugins() {
    use konoma_plugin::keys::{reserved, KeyName as PKey, KeySpec};
    use Action as A;
    // The operations that are the meaning of a layer-0-to-3 key (or of the display type's own).
    let layered = |a: Action| -> bool {
        matches!(
            a,
            A::Navigate(_)
                | A::TabNew
                | A::TabNewAfter
                | A::TabPrev
                | A::TabNext
                | A::TabGoto(_)
                | A::ToggleHelp
                | A::PasteJump
                | A::Quit
                | A::CloseTabOrQuit
                | A::ToggleTabList
                | A::ToggleFollow
                | A::PreviewBack
                | A::SearchStart
                | A::SearchNext
                | A::SearchPrev
                | A::PreviewEnterVisual
                | A::PreviewEnterVisualLine
                | A::Refresh
                | A::MarkSet
                | A::MarkJump
                | A::PreviewFileNext
                | A::PreviewFilePrev
                | A::OpenInNewTab
                | A::RequestEdit
                | A::OpenExternal
                | A::OpenGitView
                | A::PageNext
                | A::PagePrev
                | A::ToggleOutline
                | A::CycleRepresentation
                | A::ImageZoomIn
                | A::ImageZoomOut
                | A::ImageZoomReset
                | A::LinkFocusNext
                | A::LinkFocusPrev
                | A::Cancel
                | A::TreeActivate
                | A::LinkOpen
        )
    };
    // The tree's own layer 2.
    let tree_layered = |a: Action| -> bool {
        matches!(
            a,
            A::OpenGitDiffCursor
                | A::ToggleChangedFilter
                | A::SetAnchor
                | A::ResetAnchor
                | A::ToggleHidden
                | A::ToggleInfo
        )
    };
    let spec = |kp: KeyPress| -> KeySpec {
        match kp.code {
            KeyCode::Char(c) if kp.ctrl => KeySpec::ctrl(c),
            KeyCode::Char(c) => KeySpec::ch(c),
            KeyCode::Tab => KeySpec::key(PKey::Tab),
            KeyCode::BackTab => KeySpec::key(PKey::BackTab),
            KeyCode::Enter => KeySpec::key(PKey::Enter),
            KeyCode::Esc => KeySpec::key(PKey::Esc),
            KeyCode::Up => KeySpec::key(PKey::Up),
            KeyCode::Down => KeySpec::key(PKey::Down),
            KeyCode::PageUp => KeySpec::key(PKey::PageUp),
            KeyCode::PageDown => KeySpec::key(PKey::PageDown),
            KeyCode::Home => KeySpec::key(PKey::Home),
            KeyCode::End => KeySpec::key(PKey::End),
            KeyCode::Left => KeySpec::key(PKey::Left),
            KeyCode::Right => KeySpec::key(PKey::Right),
            other => panic!("unexpected key {other:?}"),
        }
    };
    // Every context that shows a pane or a list, with the table it falls under.
    #[allow(unused_mut)]
    let mut contexts: Vec<(Context, &str)> = vec![
        (Context::Tree, "tree"),
        (Context::Text, "text"),
        (Context::Document, "document"),
        (Context::Image, "image"),
        (Context::Table, "table"),
    ];
    #[cfg(feature = "git")]
    {
        contexts.push((Context::Diff, "diff"));
        for b in Backend::ALL {
            for s in Screen::ALL {
                // The commit detail is a text view and the graph picker a small list of its own.
                if !matches!(s, Screen::Detail) {
                    contexts.push((Context::Screen(b, s), "list"));
                }
            }
        }
    }
    let m = KeyMap::defaults(KeyScheme::Vim);
    let mut checked = 0;
    let mut missing: Vec<String> = Vec::new();
    for (ctx, name) in contexts {
        let table: Vec<KeySpec> = reserved(name).into_iter().map(|(k, _)| k).collect();
        let mut check = |kp: KeyPress, why: String| {
            checked += 1;
            if !table.contains(&spec(kp)) {
                missing.push(format!(
                    "{}: `{}` ({why}) is not reserved for plugins in `{name}`",
                    ctx.name(),
                    kp.label()
                ));
            }
        };
        for (kp, a) in m.bound_actions(ctx) {
            if layered(a) || (ctx == Context::Tree && tree_layered(a)) {
                check(kp, format!("{a:?}"));
            }
        }
        for (kp, id) in m.leader_keys_of(ctx) {
            check(kp, format!("the {id:?} menu"));
        }
        for f in crate::ops::keys::fixed_bindings(ctx) {
            check(f.key, format!("fixed, {:?}", f.action));
        }
    }
    assert!(missing.is_empty(), "{}", missing.join("\n"));
    // Not vacuous: the walk really visited keys of every kind.
    assert!(checked > 150, "only {checked} keys were checked");
}
