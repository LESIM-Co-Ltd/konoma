//! The shared `Diff` context and the shared `List` view type against the host's tables: whatever
//! the shared tables answer, the host must answer, under both schemes.
//!
//! The diff is one context in both hosts (`[keys.diff]`). The list screens are not: the host's
//! are the per-backend `Context::Screen(backend, screen)` contexts whose view type is `List`, so
//! the shared side is the layer-2 table (`CoreViewType::List`) only.

use super::*;
use konoma_host_core::keymap::{CoreContext, CoreKeys, CoreMenu, CoreResolution};

fn probes() -> Vec<KeyPress> {
    let mut v: Vec<KeyPress> = (0x20u8..0x7f)
        .map(|b| KeyPress::ch(char::from(b)))
        .collect();
    v.extend(('a'..='z').map(KeyPress::ctrl_ch));
    for code in [
        KeyCode::Enter,
        KeyCode::Tab,
        KeyCode::Esc,
        KeyCode::Up,
        KeyCode::Down,
        KeyCode::Left,
        KeyCode::Right,
        KeyCode::Home,
        KeyCode::End,
        KeyCode::PageUp,
        KeyCode::PageDown,
    ] {
        v.push(KeyPress::key(code));
    }
    v
}

fn cfg(tables: &[(&str, &[(&str, &str)])]) -> KeysFileConfig {
    KeysFileConfig {
        surfaces: tables
            .iter()
            .map(|(n, es)| {
                (
                    n.to_string(),
                    es.iter()
                        .map(|(k, a)| (k.to_string(), a.to_string()))
                        .collect(),
                )
            })
            .collect(),
    }
}

fn expected(r: CoreResolution) -> Resolution {
    match r {
        CoreResolution::Action(a) => Resolution::Action(a.into()),
        CoreResolution::Menu(CoreMenu::Actions) => Resolution::EnterLeader(LeaderId::Actions),
        CoreResolution::Menu(CoreMenu::Copy) => Resolution::EnterLeader(LeaderId::Copy),
        CoreResolution::Menu(CoreMenu::TableCopy) => Resolution::EnterLeader(LeaderId::TableCopy),
        CoreResolution::Menu(CoreMenu::Options) => Resolution::EnterLeader(LeaderId::Options),
        CoreResolution::Unbound => Resolution::Unbound,
    }
}

/// Without the `git` feature the host has no page turn on the diff (`J` / `K`).
fn host_lacks(kp: KeyPress) -> bool {
    !cfg!(feature = "git") && (kp == KeyPress::ch('J') || kp == KeyPress::ch('K'))
}

/// Compare the shared diff context with the host's `Context::Diff` for every probe key.
fn assert_diff_agrees(label: &str, scheme: KeyScheme, c: &KeysFileConfig) -> usize {
    let host = KeyMap::from_config(scheme, c);
    let shared = CoreKeys::from_config(scheme, c);
    let mut answered = 0;
    for kp in probes() {
        let s = shared.resolve(CoreContext::Diff, kp);
        let h = host.resolve(Context::Diff, None, kp);
        if s == CoreResolution::Unbound {
            // The host's own keys (`x` `!` `n` `N` `R` ...) are fine; a shared command or menu the
            // shared tables do not know is not.
            match h {
                Resolution::Action(a) => assert!(
                    a.core().is_none(),
                    "{label}/{scheme:?}: {kp:?}: host runs the shared command {a:?} where the shared tables leave the key unbound"
                ),
                Resolution::EnterLeader(l) => panic!(
                    "{label}/{scheme:?}: {kp:?}: host opens the menu {l:?} where the shared tables leave the key unbound"
                ),
                _ => {}
            }
        } else if !host_lacks(kp) {
            answered += 1;
            assert_eq!(
                h,
                expected(s),
                "{label}/{scheme:?}: {kp:?}: host and shared tables disagree"
            );
        }
    }
    answered
}

#[test]
fn the_host_diff_and_the_shared_diff_agree_by_default_and_when_remapped() {
    let scenarios = [
        ("defaults", cfg(&[])),
        (
            "layout-key-moved",
            cfg(&[("diff", &[("x", "cycle_diff_layout")])]),
        ),
        ("layout-key-noop", cfg(&[("diff", &[("s", "noop")])])),
        (
            "inherited-noop",
            cfg(&[("diff", &[("j", "noop"), ("g", "noop")])]),
        ),
        (
            "fixed-keys-ignored",
            cfg(&[("diff", &[("enter", "page_next"), ("esc", "quit")])]),
        ),
        (
            "global-rebind",
            cfg(&[("global", &[("X", "page_next"), ("Q", "noop")])]),
        ),
    ];
    for (label, c) in &scenarios {
        for scheme in [KeyScheme::Vim, KeyScheme::Less] {
            assert!(
                assert_diff_agrees(label, scheme, c) > 20,
                "{label}: too few keys compared"
            );
        }
    }
}

#[test]
fn the_diff_layout_key_is_s_by_default_in_both_hosts() {
    for scheme in [KeyScheme::Vim, KeyScheme::Less] {
        let host = KeyMap::from_config(scheme, &cfg(&[]));
        let shared = CoreKeys::from_config(scheme, &cfg(&[]));
        assert_eq!(
            host.resolve(Context::Diff, None, KeyPress::ch('s')),
            Resolution::Action(Action::CycleDiffLayout)
        );
        assert_eq!(
            shared.resolve(CoreContext::Diff, KeyPress::ch('s')),
            CoreResolution::Action(konoma_host_core::keymap::CoreAction::CycleDiffLayout)
        );
    }
}

/// The host's diff tables are the shared ones plus host-only keys: the shared layer-2 and own
/// tables are contained in the host's, entry for entry.
#[test]
fn the_host_diff_tables_contain_the_shared_diff_tables() {
    use konoma_host_core::keymap::{own_table, type_table, CoreViewType};
    for scheme in [KeyScheme::Vim, KeyScheme::Less] {
        let host = KeyMap::defaults(scheme);
        let ty = &host.types[&ViewType::Diff];
        let own = &host.contexts[&Context::Diff];
        let shared_ty = type_table(CoreViewType::Diff, scheme);
        let shared_own = own_table(CoreContext::Diff);
        assert!(shared_ty.len() > 10 && shared_own.len() >= 5);
        for (kp, b) in shared_ty {
            if host_lacks(kp) {
                continue;
            }
            assert_eq!(
                ty.get(&kp),
                Some(&Binding::from(b)),
                "{scheme:?} type {kp:?}"
            );
        }
        for (kp, b) in shared_own {
            // `Binding::from` of a host operation the host cannot run is filtered by `host_table`.
            if let Some(h) = own.get(&kp) {
                assert_eq!(h, &Binding::from(b), "{scheme:?} own {kp:?}");
            } else {
                panic!("{scheme:?}: the host's diff table lacks the shared key {kp:?}");
            }
        }
    }
}

/// The host's `,` menu of the diff has the shared items in order, after the follow scope that
/// only the host has.
#[test]
fn the_host_diff_options_menu_ends_with_the_shared_items() {
    let shared = CoreKeys::defaults(KeyScheme::Vim);
    let shared_items: Vec<_> = shared
        .menu(CoreContext::Diff, CoreMenu::Options)
        .unwrap()
        .items
        .iter()
        .map(|i| (i.key, Action::from(i.action)))
        .collect();
    assert!(!shared_items.is_empty());
    let host = KeyMap::defaults(KeyScheme::Vim);
    let host_items: Vec<_> = host
        .menu(Context::Diff, LeaderId::Options)
        .unwrap()
        .items
        .iter()
        .map(|i| (i.key, i.action))
        .collect();
    assert!(
        host_items.ends_with(&shared_items),
        "{host_items:?} vs {shared_items:?}"
    );
}

#[cfg(feature = "git")]
mod list {
    use super::*;
    use konoma_host_core::keymap::CoreViewType;

    /// The host screens whose view type is `List` (every screen but the commit detail, which is
    /// a text view). The changes screen is among them.
    fn list_screens() -> Vec<Context> {
        Backend::ALL
            .iter()
            .flat_map(|&b| {
                Screen::ALL
                    .iter()
                    .filter(|s| !matches!(s, Screen::Detail))
                    .map(move |&s| Context::Screen(b, s))
            })
            .collect()
    }

    #[test]
    fn the_host_list_view_type_is_the_shared_one_for_every_key_and_scheme() {
        for scheme in [KeyScheme::Vim, KeyScheme::Less] {
            let host = KeyMap::from_config(scheme, &cfg(&[]));
            let shared = CoreKeys::from_config(scheme, &cfg(&[]));
            let host_t = &host.types[&ViewType::List];
            let shared_t = &shared.types[&CoreViewType::List];
            assert_eq!(shared_t.len(), 2, "the shared list table is g and G");
            for kp in probes() {
                assert_eq!(
                    host_t.get(&kp).cloned(),
                    shared_t.get(&kp).cloned().map(Binding::from),
                    "{scheme:?} {kp:?}"
                );
            }
        }
    }

    #[test]
    fn every_host_list_screen_answers_the_shared_list_keys_unless_its_own_table_binds_them() {
        for scheme in [KeyScheme::Vim, KeyScheme::Less] {
            let host = KeyMap::from_config(scheme, &cfg(&[]));
            let shared = CoreKeys::from_config(scheme, &cfg(&[]));
            let shared_t = &shared.types[&CoreViewType::List];
            let screens = list_screens();
            assert!(screens
                .iter()
                .any(|c| matches!(c, Context::Screen(_, Screen::Changes))));
            let (mut compared, mut overridden) = (0, 0);
            for hc in screens {
                assert_eq!(hc.view_types(), &[ViewType::List], "{hc:?}");
                for (&kp, b) in shared_t {
                    // Layer 4 (the screen's own table) wins over layer 2 by design; read it
                    // instead of assuming no screen binds the shared keys.
                    if host.contexts.get(&hc).is_some_and(|t| t.contains_key(&kp)) {
                        overridden += 1;
                        continue;
                    }
                    compared += 1;
                    assert_eq!(
                        host.resolve(hc, None, kp),
                        expected(CoreResolution::Action(match b {
                            konoma_host_core::keymap::CoreBinding::Run(a) => *a,
                            other => panic!("unexpected shared binding {other:?}"),
                        })),
                        "{scheme:?} {hc:?} {kp:?}"
                    );
                }
            }
            // 2 keys x (2 backends x 6 list screens); no screen binds `g` / `G` itself.
            assert_eq!(overridden, 0, "{scheme:?}: a screen overrides g/G");
            assert_eq!(compared, 2 * list_screens().len());
            assert!(
                list_screens().len() >= 12,
                "{scheme:?}: missing list screens"
            );
            assert!(compared >= 24, "{scheme:?}: too few list comparisons");
        }
    }

    #[test]
    fn a_remap_of_a_list_key_in_one_screen_stays_in_that_screen() {
        let host = KeyMap::from_config(KeyScheme::Vim, &cfg(&[("git.log", &[("g", "noop")])]));
        assert_eq!(
            host.resolve(
                Context::Screen(Backend::Git, Screen::Log),
                None,
                KeyPress::ch('g')
            ),
            Resolution::Unbound
        );
        assert_eq!(
            host.resolve(
                Context::Screen(Backend::Git, Screen::Branches),
                None,
                KeyPress::ch('g')
            ),
            Resolution::Action(Action::Navigate(Motion::Top))
        );
    }
}

#[cfg(feature = "git")]
mod diff_help_footer {
    use super::*;
    use crate::app::App;
    use crate::config::Config;
    use konoma_host_core::help::{self, CoreLookup, HelpFacts};
    use konoma_host_core::preview_plan::{footer_plan, PreviewState, PvFoot};
    use konoma_vcs_core::git2;

    /// The exact exclusions from the host's rendered diff help and footer. Some shell controls
    /// have shared key bindings, but belong to the full host's tabs, clipboard and follow UI.
    struct HostOnly {
        actions: &'static [Action],
        literal: Option<&'static str>,
        reason: &'static str,
    }

    const HOST_ONLY: &[HostOnly] = &[
        HostOnly {
            actions: &[Action::GitDiffDiscard],
            literal: None,
            reason: "x: Discarding a working-copy change belongs to the host's write workflow.",
        },
        HostOnly {
            actions: &[Action::GitLaunchTool],
            literal: None,
            reason: "!: Launching the external VCS tool requires the host's process integration.",
        },
        HostOnly {
            actions: &[Action::JumpNextChange, Action::JumpPrevChange],
            literal: None,
            reason: "n/N: Changed-file navigation requires the host's repository and tree state.",
        },
        HostOnly {
            actions: &[Action::CycleDiffView],
            literal: None,
            reason: "R: Representation cycling requires the host's file preview pipeline.",
        },
        HostOnly {
            actions: &[Action::DiffNextChange, Action::DiffPrevChange],
            literal: None,
            reason: "Ctrl-n/Ctrl-p: Hunk navigation requires the host's diff cursor state.",
        },
        HostOnly {
            actions: &[Action::OpenGitView],
            literal: None,
            reason: "p: Opening the VCS hub requires the host's screen stack.",
        },
        HostOnly {
            actions: &[Action::Refresh],
            literal: None,
            reason: "r: Refreshing the current file requires the host's reload pipeline.",
        },
        HostOnly {
            actions: &[Action::ToggleFollowDiffScope],
            literal: None,
            reason:
                "Unbound follow scope: Switching follow scope requires the host's tree watcher.",
        },
        HostOnly {
            actions: &[Action::TabNew],
            literal: None,
            reason: "Ctrl-t: Opening a new tab requires the host's tab manager.",
        },
        HostOnly {
            actions: &[Action::TabNewAfter],
            literal: None,
            reason: "t: Inserting a tab requires the host's tab manager.",
        },
        HostOnly {
            actions: &[Action::TabPrev, Action::TabNext],
            literal: None,
            reason: "[/]: Switching tabs requires the host's tab manager.",
        },
        HostOnly {
            actions: &[],
            literal: Some("1 - 9"),
            reason: "1-9: Direct tab selection requires the host's tab manager.",
        },
        HostOnly {
            actions: &[Action::ToggleTabList],
            literal: None,
            reason: "T: Listing tabs requires the host's tab manager.",
        },
        HostOnly {
            actions: &[Action::PasteJump],
            literal: None,
            reason: "P: Opening a clipboard path requires the host's clipboard navigation.",
        },
        HostOnly {
            actions: &[Action::ToggleFollow],
            literal: None,
            reason: "F: Following file updates requires the host's tree watcher.",
        },
    ];

    fn excluded(actions: &[Action], keys: &str) -> bool {
        if actions.is_empty() {
            return HOST_ONLY.iter().any(|op| op.literal == Some(keys));
        }
        let count = actions
            .iter()
            .filter(|a| HOST_ONLY.iter().any(|op| op.actions.contains(a)))
            .count();
        assert!(
            count == 0 || count == actions.len(),
            "mixed shared/host-only row: {keys}"
        );
        count != 0
    }

    #[test]
    fn the_shared_diff_help_and_footer_equal_the_host_reference() {
        assert_eq!(
            HOST_ONLY.len(),
            15,
            "the exclusion list must not grow silently"
        );
        assert_eq!(
            HOST_ONLY.iter().map(|op| op.actions.len()).sum::<usize>(),
            17
        );
        assert!(HOST_ONLY.iter().all(|op| !op.reason.is_empty()));
        let dir = crate::test_support::unique_tmp("diff_help_footer_agreement");
        std::fs::create_dir_all(&dir).unwrap();
        crate::test_support::init_git_repo(&dir);
        let path = dir.join("ordinary.txt");
        std::fs::write(&path, "before\n").unwrap();
        let repo = git2::Repository::open(&dir).unwrap();
        let mut index = repo.index().unwrap();
        index
            .add_path(std::path::Path::new("ordinary.txt"))
            .unwrap();
        index.write().unwrap();
        let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
        let signature =
            git2::Signature::new("Test", "test@example.invalid", &git2::Time::new(0, 0)).unwrap();
        repo.commit(Some("HEAD"), &signature, &signature, "seed", &tree, &[])
            .unwrap();
        std::fs::write(&path, "after\n").unwrap();
        let root = std::fs::canonicalize(&dir).unwrap();

        for lang in ["en", "jp"] {
            for scheme in ["vim", "less"] {
                let mut config = Config::default();
                config.ui.lang = lang.into();
                config.ui.keys = scheme.into();
                config.ui.diff_view = "source".into();
                config.git.diff = "unified".into();
                let shared = CoreKeys::from_keys_config(scheme_from_str(scheme), &config.keys);
                let mut app = App::new(root.clone(), config).unwrap();
                app.open_git_diff(&root.join("ordinary.txt"));
                assert_eq!(app.surface(), Surface::PreviewGitDiff);
                assert_eq!(app.diff_view_for_test(), crate::app::DiffView::Source);
                assert!(!app.diff_is_split(u16::MAX));
                assert!(!app.diff_footer_is_media_or_summary());
                assert!(
                    app.diff_view_cycle_hint().is_some(),
                    "R switches source to preview"
                );

                let lookup = CoreLookup {
                    keys: &shared,
                    ctx: CoreContext::Diff,
                };
                // Ordinary source text has no media pages; J/K do not act in this state.
                let facts = HelpFacts {
                    off: vec![
                        help::HelpOp::Act(konoma_host_core::keymap::CoreAction::PageNext),
                        help::HelpOp::Act(konoma_host_core::keymap::CoreAction::PagePrev),
                    ],
                    ..HelpFacts::default()
                };
                let host_rows: Vec<_> = crate::ops::help::rows_for(&app, Context::Diff)
                    .into_iter()
                    .filter(|r| !excluded(&r.actions, &r.keys_text))
                    .map(|r| (r.keys_text, r.desc))
                    .collect();
                let shared_rows: Vec<_> = help::rows_of(CoreContext::Diff)
                    .iter()
                    .filter_map(|r| {
                        help::render_row(*r, CoreContext::Diff, &facts, &lookup, app.lang)
                    })
                    .map(|r| (r.keys_text, r.desc))
                    .collect();
                assert_eq!(shared_rows, host_rows, "{lang}/{scheme}: diff help");
                assert!(host_rows.len() >= 11, "{lang}/{scheme}: too few help rows");

                let host_footer: Vec<_> = crate::ops::footer::render_tokens(
                    &app,
                    Context::Diff,
                    &crate::ops::footer::plan(&app, Context::Diff),
                )
                .into_iter()
                .filter(|t| !excluded(&t.actions, ""))
                .map(|t| t.text)
                .collect();
                let plan: Vec<_> = footer_plan(CoreContext::Diff, &PreviewState::default())
                    .into_iter()
                    .map(|f| match f {
                        PvFoot::Core(c) => c,
                        other => panic!("unexpected diff footer entry {other:?}"),
                    })
                    .collect();
                let shared_footer: Vec<_> =
                    help::footer_tokens(&plan, CoreContext::Diff, &facts, &lookup, app.lang)
                        .into_iter()
                        .map(|t| t.text)
                        .collect();
                assert!(
                    host_footer.len() >= 7,
                    "{lang}/{scheme}: too few footer hints"
                );
                assert_eq!(shared_footer, host_footer, "{lang}/{scheme}: diff footer");
            }
        }
    }
}
