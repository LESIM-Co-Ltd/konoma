//! Graph comparisons include the unscrolled legend and every body cell.

use super::*;

fn fixture() -> TmpDir {
    let dir = repo(1);
    let repository = git2::Repository::open(&dir).unwrap();
    let base = repository.head().unwrap().peel_to_commit().unwrap();
    let tree = base.tree().unwrap();
    let make =
        |reference: Option<&str>, subject: &str, parents: &[&git2::Commit<'_>], minute: i64| {
            let signature = git2::Signature::new(
                "作者 Author",
                "fixture@example.invalid",
                &git2::Time::new(1_577_836_800 + minute * 60, 0),
            )
            .unwrap();
            let id = repository
                .commit(reference, &signature, &signature, subject, &tree, parents)
                .unwrap();
            repository.find_commit(id).unwrap()
        };
    let side = make(
        Some("refs/heads/priority-branch-with-a-long-name"),
        "Side 日本語\n\nBody",
        &[&base],
        1,
    );
    let head = make(Some("HEAD"), "Main commit", &[&base], 2);
    let merged = make(Some("HEAD"), "Merge 日本語\n\nBody", &[&head, &side], 3);
    repository
        .branch("second-branch-with-a-long-name", &merged, false)
        .unwrap();
    // Include a staged tracked file, an unstaged edit and an untracked file.
    std::fs::write(dir.join("tracked"), "staged\n").unwrap();
    let mut index = repository.index().unwrap();
    index.add_path(std::path::Path::new("tracked")).unwrap();
    index.write().unwrap();
    std::fs::write(dir.join("tracked"), "unstaged 日本語\n").unwrap();
    std::fs::write(dir.join("untracked"), "untracked\n").unwrap();
    dir
}

fn graph_pair(
    root: &std::path::Path,
    lang: &str,
    align: &str,
    size: (u16, u16),
    cap: usize,
    priority: bool,
    icons: bool,
) -> (Sim, Sim) {
    let (width, height) = size;
    let mut cfg = plugin_config("git", "vcs_graph_parity", lang);
    cfg.ui.commit_meta_align = align.into();
    cfg.ui.graph_max_branches = cap;
    cfg.ui.icons = icons;
    if priority {
        cfg.ui.graph_base_branches = vec![
            "missing-branch".into(),
            "priority-branch-with-a-long-name".into(),
        ];
    }
    let settings = cfg.plugins.entry("git".into()).or_default();
    settings.insert(
        "commit_meta_align".into(),
        toml::Value::String(align.into()),
    );
    settings.insert(
        "graph_max_branches".into(),
        toml::Value::Integer(cap as i64),
    );
    settings.insert(
        "graph_base_branches".into(),
        toml::Value::Array(
            cfg.ui
                .graph_base_branches
                .iter()
                .cloned()
                .map(toml::Value::String)
                .collect(),
        ),
    );
    let (mut old, mut new) = pair_config(root, "git", cfg, width, height);
    old.app.open_git_graph();
    for _ in 0..2 {
        new.press(KeyCode::Tab, KeyModifiers::NONE);
        drain(&mut new);
    }
    assert!(old.app.is_git_graph());
    assert_eq!(new.app.project_frame().unwrap().key.screen, "graph");
    (old, new)
}

#[test]
fn e2e_vcs_graph_cells_open_move_filter_inline_width_legend_cap_and_priority() {
    let dir = fixture();
    for lang in ["en", "ja"] {
        for width in [80, 160] {
            for (align, cap, priority, icons) in [
                ("right", 12, false, true),
                ("inline", 12, false, true),
                ("right", 1, false, false),
                ("right", 0, true, false),
            ] {
                let (mut old, mut new) =
                    graph_pair(&dir, lang, align, (width, 16), cap, priority, icons);
                assert!(old
                    .app
                    .git_graph_rows()
                    .iter()
                    .any(|r| r.commit.is_none() && !r.worktree));
                same(&mut old, &mut new, width, 16);
                for _ in 0..2 {
                    old.key('j');
                    new.key('j');
                    drain(&mut new);
                    same(&mut old, &mut new, width, 16);
                }
                assert!(
                    old.app.git_graph_sel() > 2,
                    "two moves must cross a connector"
                );
                for query in ["日本語", "Main", "nothing-matches", "│"] {
                    old.key('/');
                    new.key('/');
                    for c in query.chars() {
                        old.key(c);
                        new.key(c);
                    }
                    old.enter();
                    new.enter();
                    drain(&mut new);
                    same(&mut old, &mut new, width, 16);
                }
            }
        }
    }
    for width in [80, 160] {
        for height in [5, 6, 7, 8] {
            let (mut old, mut new) =
                graph_pair(&dir, "en", "right", (width, height), 12, false, true);
            same(&mut old, &mut new, width, height);
            for key in ['G', 'g'] {
                old.key(key);
                new.key(key);
                drain(&mut new);
                same(&mut old, &mut new, width, height);
            }
        }
    }
}

#[test]
fn e2e_vcs_graph_base_reload_detail_copy_and_cursor_return_match_the_hub() {
    use konoma_host_core::clipboard::{clear_test_clipboard, get_test_clipboard};
    let dir = fixture();
    for lang in ["en", "ja"] {
        let (mut old, mut new) = graph_pair(&dir, lang, "right", (160, 16), 12, false, true);
        old.key('s');
        new.key('s');
        drain(&mut new);
        assert_eq!(old.app.flash, new.app.flash);
        same(&mut old, &mut new, 160, 16);
        // The worktree row has no copy items, following the log's empty-row rule.
        new.key('y');
        assert_eq!(new.app.pending_leader, None);
        for worktree in [true, false] {
            if !worktree {
                old.key('j');
                new.key('j');
                drain(&mut new);
            }
            if !worktree {
                for key in ['s', 'h', 't', 'm', 'a', 'd'] {
                    clear_test_clipboard();
                    old.key('y');
                    old.key(key);
                    let text = get_test_clipboard().unwrap();
                    if key == 'm' {
                        assert!(text.contains("\n\nBody"));
                    }
                    let flash = old.app.flash.clone();
                    clear_test_clipboard();
                    new.key('y');
                    new.key(key);
                    drain(&mut new);
                    assert_eq!(get_test_clipboard(), Some(text));
                    assert_eq!(new.app.flash, flash);
                }
                let side = old
                    .app
                    .git_graph_rows()
                    .iter()
                    .find(|row| row.subject == "Side 日本語")
                    .unwrap()
                    .commit
                    .clone()
                    .unwrap();
                for _ in 0..old.app.git_graph_rows().len() {
                    if old.app.git_graph_selected_row().unwrap().commit.as_ref() == Some(&side) {
                        break;
                    }
                    old.key('j');
                    new.key('j');
                    drain(&mut new);
                }
                assert_eq!(new.app.project_act_context().selected.unwrap().value, side);
                old.key('s');
                new.key('s');
                drain(&mut new);
                assert_eq!(new.app.project_act_context().selected.unwrap().value, side);
                assert_eq!(old.app.flash, new.app.flash);
                same(&mut old, &mut new, 160, 16);
                old.key('x');
                new.key('x');
                drain(&mut new);
                assert_eq!(old.app.flash, new.app.flash);
                same(&mut old, &mut new, 160, 16);
                old.key('x');
                new.key('x');
                drain(&mut new);
                assert_eq!(
                    old.app.flash, new.app.flash,
                    "clearing an unset base is silent"
                );
                let id = new.app.project_act_context().selected.unwrap().value;
                old.key('r');
                new.key('r');
                drain(&mut new);
                assert_eq!(new.app.project_act_context().selected.unwrap().value, id);
                same(&mut old, &mut new, 160, 16);
            }
            for key in [KeyCode::Enter, KeyCode::Char('l')] {
                old.press(key, KeyModifiers::NONE);
                new.press(key, KeyModifiers::NONE);
                drain(&mut new);
                assert!(new.app.project_pushed());
                same(&mut old, &mut new, 160, 16);
                old.key('q');
                new.key('q');
                drain(&mut new);
                same(&mut old, &mut new, 160, 16);
            }
        }
    }
}

#[test]
fn e2e_vcs_graph_base_visible_order_and_view_cache_are_per_tab() {
    let dir = fixture();
    let (_, mut new) = graph_pair(&dir, "en", "right", (160, 16), 1, false, true);
    let first_tab = new.app.tab.id;
    new.key('j');
    new.key('s');
    drain(&mut new);
    let pinned = new.app.project_content().unwrap();
    let ProjectContent::List(pinned) = pinned.as_ref() else {
        panic!()
    };
    assert!(pinned.title.as_ref().unwrap().plain().contains("base:"));
    assert!(pinned
        .bottom
        .as_ref()
        .unwrap()
        .chips
        .last()
        .unwrap()
        .plain()
        .contains("hidden"));
    new.app.tab_new().unwrap();
    assert_ne!(first_tab, new.app.tab.id);
    new.app.tab.projects.wait_for_result();
    new.app.poll_projects();
    new.key('p');
    drain(&mut new);
    for _ in 0..2 {
        new.press(KeyCode::Tab, KeyModifiers::NONE);
        drain(&mut new);
    }
    let content = new.app.project_content().unwrap();
    let ProjectContent::List(unpinned) = content.as_ref() else {
        panic!()
    };
    assert!(!unpinned.title.as_ref().unwrap().plain().contains("base:"));
    new.key('r');
    drain(&mut new);
    new.app.tab_goto(0);
    drain(&mut new);
    let content = new.app.project_content().unwrap();
    let ProjectContent::List(restored) = content.as_ref() else {
        panic!()
    };
    assert_eq!(restored, pinned);
}

fn hints_match(old: &Sim, new: &Sim, jj: bool) {
    use crate::i18n::{tr, Msg};
    use crate::keymap::{Action, Context};
    let legacy = crate::ops::help::sections_for(&old.app, old.app.key_context().unwrap());
    let actual = crate::ops::help::sections_for(&new.app, Context::Canvas);
    let title = tr(
        old.app.lang,
        if jj {
            Msg::JjGraphLabel
        } else {
            Msg::GitGraphLabel
        },
    );
    assert!(
        actual.iter().any(|section| section.title == title),
        "{title}"
    );
    let mut expected_help = Vec::new();
    for section in &legacy {
        for (_, description) in &section.rows {
            if [
                Msg::GitMoveCommit,
                Msg::TopBottom,
                Msg::GitDetail,
                Msg::GraphSetBaseHelp,
                Msg::GraphClearBaseHelp,
                Msg::JjGraphAllRow,
                Msg::ListFilterHelp,
                Msg::Refresh,
                Msg::GitScreensHelp,
                Msg::JjScreensHelp,
                Msg::BackToChanges,
            ]
            .into_iter()
            .any(|msg| description == tr(old.app.lang, msg))
            {
                expected_help.push(description.clone());
            }
        }
    }
    let graph_help = actual
        .iter()
        .find(|section| section.title == title)
        .unwrap();
    let mut expected_titles = vec![title.to_string()];
    for (leader, key) in [
        (crate::keymap::LeaderId::Copy, "y"),
        (crate::keymap::LeaderId::Actions, "Space"),
    ] {
        if let Some(section) =
            crate::ui::help::leader_section(&new.app, Context::Canvas, leader, key)
        {
            expected_titles.push(section.title);
        }
    }
    assert_eq!(
        actual
            .iter()
            .map(|section| section.title.clone())
            .collect::<Vec<_>>(),
        expected_titles
    );
    let mut actual_help: Vec<_> = graph_help
        .rows
        .iter()
        .map(|(_, text)| text.clone())
        .collect();
    expected_help.sort();
    actual_help.sort();
    assert_eq!(
        actual_help, expected_help,
        "graph help must have neither missing nor extra rows"
    );
    let legacy = crate::ops::footer::render_tokens(
        &old.app,
        old.app.key_context().unwrap(),
        &crate::ops::footer::plan(&old.app, old.app.key_context().unwrap()),
    );
    let actual = crate::ops::footer::tokens(&new.app, Context::Canvas);
    let mut expected_footer = Vec::new();
    for token in legacy
        .iter()
        .filter(|token| !token.actions.contains(&Action::GitGraphOpenPicker))
    {
        let label = token.text.split_once(':').unwrap().1;
        expected_footer.push(label.to_string());
    }
    let actual_footer: Vec<_> = actual
        .iter()
        .map(|token| token.split_once(':').unwrap().1.to_string())
        .collect();
    assert_eq!(
        actual_footer, expected_footer,
        "footer must have neither missing nor extra tokens"
    );
    let has_key = |key: &str| {
        actual.iter().any(|token| {
            token
                .split_once(':')
                .is_some_and(|(keys, _)| keys.split('/').any(|candidate| candidate == key))
        })
    };
    assert!(!has_key("b"));
    if jj {
        assert!(!has_key("s") && !has_key("x"));
    }
}

#[test]
fn e2e_vcs_graph_help_and_footer_follow_legacy_labels_and_priorities_in_both_languages() {
    let dir = fixture();
    for lang in ["en", "ja"] {
        let (old, new) = graph_pair(&dir, lang, "right", (160, 16), 12, false, true);
        hints_match(&old, &new, false);
    }
}

#[test]
fn e2e_vcs_graph_jj_cells_title_all_range_flash_and_no_base_keys() {
    if !crate::vcs::jj::available() {
        return;
    }
    let dir = crate::test_support::unique_tmp("vcs_graph_jj");
    std::fs::create_dir_all(&dir).unwrap();
    let dir = dir.into_canonical().unwrap();
    jj(&dir, &["git", "init", "--no-colocate", "."]);
    // A local remote pins trunk so the default revset is narrower than all().
    let remote = crate::test_support::unique_tmp("vcs_graph_jj_remote");
    git2::Repository::init_bare(&remote).unwrap();
    jj(
        &dir,
        &["git", "remote", "add", "origin", remote.to_str().unwrap()],
    );
    for message in ["c1", "c2", "Trunk 日本語"] {
        jj(&dir, &["commit", "-m", message]);
    }
    jj(&dir, &["bookmark", "create", "-r", "@-", "main"]);
    jj(
        &dir,
        &["git", "push", "--remote", "origin", "--bookmark", "main"],
    );
    jj(&dir, &["describe", "-m", "Next 日本語"]);
    let _preference = Preference::pin("jj");
    for lang in ["en", "ja"] {
        let cfg = plugin_config("jj", "vcs_graph_jj", lang);
        let (mut old, mut new) = pair_config(&dir, "jj", cfg, 160, 16);
        old.app.open_git_graph();
        for _ in 0..2 {
            new.press(KeyCode::Tab, KeyModifiers::NONE);
            drain(&mut new);
        }
        let content = new.app.project_content().unwrap();
        let ProjectContent::List(list) = content.as_ref() else {
            panic!("list required")
        };
        assert!(list.title.as_ref().unwrap().plain().starts_with("jj graph"));
        same(&mut old, &mut new, 160, 16);
        hints_match(&old, &new, true);
        let original = rows(&new);
        for all in [true, false] {
            old.key('a');
            new.key('a');
            drain(&mut new);
            assert_eq!(new.app.flash, old.app.flash);
            same(&mut old, &mut new, 160, 16);
            if all {
                assert!(rows(&new).len() > original.len());
                let expanded = rows(&new);
                new.app.tab_new().unwrap();
                new.app.tab.projects.wait_for_result();
                new.app.poll_projects();
                new.key('p');
                drain(&mut new);
                for _ in 0..2 {
                    new.press(KeyCode::Tab, KeyModifiers::NONE);
                    drain(&mut new);
                }
                assert_eq!(rows(&new), original);
                new.app.tab_goto(0);
                drain(&mut new);
                assert_eq!(rows(&new), expanded);
            } else {
                assert_eq!(rows(&new), original);
            }
        }
        let before = rows(&new);
        for key in ['s', 'x'] {
            new.key(key);
            drain(&mut new);
            assert_eq!(rows(&new), before);
        }
    }
}

fn graph_left_returns(root: &std::path::Path, id: &str) {
    let _preference = Preference::pin(id);
    for key in [KeyCode::Char('h'), KeyCode::Left] {
        let cfg = plugin_config(id, &format!("vcs_graph_left_{id}"), "en");
        let (mut old, mut new) = pair_config(root, id, cfg, 100, 16);
        let previous = new.app.tab.mode;
        old.app.open_git_graph();
        for _ in 0..2 {
            new.press(KeyCode::Tab, KeyModifiers::NONE);
            drain(&mut new);
        }
        old.press(key, KeyModifiers::NONE);
        if key == KeyCode::Left {
            // At 7f02d00 the old hub's fixed Left is a no-op; compare its h back action.
            assert!(old.app.is_git_graph());
            old.key('h');
        }
        new.press(key, KeyModifiers::NONE);
        drain(&mut new);
        assert!(!old.app.is_git_graph());
        assert!(old.app.is_git_view());
        assert!(!new.app.project_active(), "{id} {key:?}");
        assert_eq!(new.app.tab.mode, previous);
    }
}

#[test]
fn e2e_vcs_graph_git_h_and_left_return_like_the_old_hub() {
    let dir = fixture();
    graph_left_returns(&dir, "git");
}

#[test]
fn e2e_vcs_graph_jj_h_and_left_return_like_the_old_hub() {
    if !crate::vcs::jj::available() {
        return;
    }
    let dir = crate::test_support::unique_tmp("vcs_graph_left_jj");
    std::fs::create_dir_all(&dir).unwrap();
    let dir = dir.into_canonical().unwrap();
    jj(&dir, &["git", "init", "--no-colocate", "."]);
    jj(&dir, &["describe", "-m", "Graph 日本語"]);
    graph_left_returns(&dir, "jj");
}

#[test]
fn e2e_vcs_graph_jj_repository_tests_guard_optional_binary() {
    let source = include_str!("vcs_graph.rs");
    for name in [
        "e2e_vcs_graph_jj_cells_title_all_range_flash_and_no_base_keys",
        "e2e_vcs_graph_jj_h_and_left_return_like_the_old_hub",
    ] {
        let (_, body) = source.split_once(&format!("fn {name}() {{")).unwrap();
        let setup = body.split("let dir").next().unwrap();
        assert!(
            setup.contains("if !crate::vcs::jj::available()")
                && setup.contains("return;")
                && !setup.contains("assert!"),
            "{name} must skip when jj is absent"
        );
    }
}

#[test]
fn e2e_vcs_graph_changelog_records_tab_cache_and_shared_list_differences() {
    let notes = include_str!("../../CHANGELOG.md");
    let unreleased = notes
        .split_once("## [Unreleased]")
        .unwrap()
        .1
        .split("\n## [")
        .next()
        .unwrap();
    for difference in [
        "second tab",
        "non-selectable rows",
        "uses the keymap",
        "Closing an active or inactive tab",
    ] {
        assert!(
            unreleased.contains(difference),
            "missing graph difference: {difference}"
        );
    }
}

#[test]
fn e2e_vcs_graph_tab_close_discards_only_closed_tabs_cached_views() {
    let dir = fixture();
    for inactive in [false, true] {
        use crate::plugins::test_projects::{ObservedVcs, VcsCalls};
        use std::sync::{Arc, Mutex};
        let calls = Arc::new(Mutex::new(VcsCalls::default()));
        let token = crate::plugins::test_plugins::register(
            &format!("vcs_graph_close_{inactive}"),
            Arc::new(ObservedVcs {
                plugin: Arc::new(konoma_vcs::Git),
                calls: calls.clone(),
            }),
        );
        let mut cfg = crate::plugins::test_plugins::marker_config(&token);
        cfg.ui.lang = "en".into();
        let (_, mut new) = pair_config(&dir, "git", cfg, 160, 16);
        for _ in 0..2 {
            new.press(KeyCode::Tab, KeyModifiers::NONE);
            drain(&mut new);
        }
        let a = new.app.tab.id;
        let original = rows(&new);
        new.app.tab_new().unwrap();
        new.app.tab.projects.wait_for_result();
        new.app.poll_projects();
        new.key('p');
        drain(&mut new);
        for _ in 0..2 {
            new.press(KeyCode::Tab, KeyModifiers::NONE);
            drain(&mut new);
        }
        let b = new.app.tab.id;
        assert!(new.app.project_cached_tabs_for_test().contains(&a));
        assert!(new.app.project_cached_tabs_for_test().contains(&b));
        if inactive {
            new.app.tab_goto(0);
            new.app.toggle_tab_list();
            new.app.tab_list_move(1);
            new.app.tab_list_close_selected();
            new.app.toggle_tab_list();
        } else {
            new.app.tab_close();
        }
        drain(&mut new);
        assert_eq!(calls.lock().unwrap().closed_tabs, [b]);
        let tabs = new.app.project_cached_tabs_for_test();
        assert!(tabs.contains(&a));
        assert!(!tabs.contains(&b));
        assert_eq!(rows(&new), original);
        new.key('r');
        drain(&mut new);
        assert!(!new.app.project_cached_tabs_for_test().contains(&b));
    }
}
