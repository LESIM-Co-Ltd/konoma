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
        Surface::Sort,
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
        Surface::Sort => 10,
        Surface::Bookmarks => 11,
        Surface::Tabs => 12,
        Surface::Outline => 13,
        Surface::Info => 14,
        Surface::TableCell => 15,
        Surface::Visual => 16,
        Surface::Tree => 17,
        Surface::PreviewText => 18,
        Surface::PreviewTextVisual => 19,
        Surface::PreviewImage => 20,
        Surface::PreviewTable => 21,
        #[cfg(feature = "git")]
        Surface::BranchFilter => 22,
        #[cfg(feature = "git")]
        Surface::WorktreeFilter => 23,
        #[cfg(feature = "git")]
        Surface::GitDetail => 24,
        #[cfg(feature = "git")]
        Surface::GitLog => 25,
        #[cfg(feature = "git")]
        Surface::GitGraph => 26,
        #[cfg(feature = "git")]
        Surface::GitGraphPicker => 27,
        #[cfg(feature = "git")]
        Surface::GitBranches => 28,
        #[cfg(feature = "git")]
        Surface::GitChanges => 29,
        #[cfg(feature = "git")]
        Surface::GitWorktrees => 30,
        #[cfg(feature = "git")]
        Surface::PreviewGitDiff => 31,
    }
}

#[test]
fn the_surface_list_is_complete() {
    let all = all_surfaces();
    let mut ords: Vec<usize> = all.iter().map(|&s| surface_ordinal(s)).collect();
    ords.sort_unstable();
    let expected: Vec<usize> = if cfg!(feature = "git") {
        (0..32).collect()
    } else {
        (0..22).collect()
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
                || !(ctx == "diff" || ctx.starts_with("git.") || ctx.starts_with("jj."))
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
            cfg!(feature = "git") || !(l.starts_with("git_copy") || l.starts_with("branch_copy"))
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
            konoma_plugin::keys::KeyCode::Char(c) if k.ctrl => Some(c),
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
        // Layer 1's `Space` is the operation menu (B3b); the panel's toggle moves then.
        ("Space", "toggle in the branch panel"),
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
    // Contexts whose letters are not a cursor's keys: the sort menu is a one-shot picker with
    // its own letters (moving into the `,` menu in B3b), the bookmark list gives plain letters
    // to bookmark names, and the tab list and the info popup are small pickers.
    let pickers = [Context::Sort, Context::Bookmarks, Context::Info];
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
            ],
            "l" => vec![
                nav(Motion::Right),
                action_name(Action::TreeDescend),
                "git_open_selected_diff".to_string(),
                "git_open_detail".to_string(),
                "branch_checkout".to_string(),
                "worktree_goto".to_string(),
                "git_graph_picker_apply".to_string(),
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
            ],
            // n / N : the next / previous match or change.
            "n" => vec![
                action_name(Action::SearchNext),
                action_name(Action::JumpNextChange),
            ],
            "N" => vec![
                action_name(Action::SearchPrev),
                action_name(Action::JumpPrevChange),
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

#[test]
fn a_menu_entry_reaches_its_menu_from_any_table() {
    for scheme in [KeyScheme::Vim, KeyScheme::Less] {
        for name in ["tree", "global", "text"] {
            let cfg = cfg_from(&[(name, &[("y z", "copy_full"), ("space z", "file_delete")])]);
            let m = KeyMap::from_config(scheme, &cfg);
            assert!(m.warnings.is_empty(), "{:?}", m.warnings);
            assert_eq!(
                m.resolve_leader(LeaderId::Copy, KeyPress::ch('z')),
                Resolution::Action(Action::CopyPath(CopyKind::Full))
            );
            assert_eq!(
                m.resolve_leader(LeaderId::File, KeyPress::ch('z')),
                Resolution::Action(Action::FileDelete)
            );
        }
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
        assert_eq!(at(Context::Sort, 'n'), Resolution::Unbound);
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
            m.resolve_leader(LeaderId::File, KeyPress::ch('z')),
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
