//! Compare linear log bodies through the legacy hub and the project host.

use super::vcs_changes::{compare_body, drain, pair_config, plugin_config};
use super::*;
use crate::app::list_filter::ListId;
use crate::app::project_screens::ProjectContent;
use crate::test_support::{init_git_repo, TmpDir};
use konoma_vcs_core::git2;

#[path = "vcs_worktrees.rs"]
mod worktrees;

fn commit(root: &std::path::Path, i: usize) {
    let repo = git2::Repository::open(root).unwrap();
    let sig = git2::Signature::new(
        "作者 Author",
        "fixture@example.invalid",
        &git2::Time::new(1_577_836_800 + i as i64 * 60, 0),
    )
    .unwrap();
    let tree_id = repo.index().unwrap().write_tree().unwrap();
    let tree = repo.find_tree(tree_id).unwrap();
    let parent = repo.head().ok().map(|h| h.peel_to_commit().unwrap());
    let subject = if i.is_multiple_of(2) {
        format!("{i} 日本語の長い件名 {} Summary", "長文 abc ".repeat(12))
    } else {
        format!("{i} 日本語 Summary")
    };
    repo.commit(
        Some("HEAD"),
        &sig,
        &sig,
        &subject,
        &tree,
        &parent.iter().collect::<Vec<_>>(),
    )
    .unwrap();
}

fn repo(count: usize) -> TmpDir {
    let dir = crate::test_support::unique_tmp("vcs_log_parity");
    std::fs::create_dir_all(&dir).unwrap();
    init_git_repo(&dir);
    for i in 0..count {
        commit(&dir, i);
    }
    dir.into_canonical().unwrap()
}

fn pair(
    root: &std::path::Path,
    id: &str,
    lang: &str,
    align: &str,
    width: u16,
    height: u16,
) -> (Sim, Sim) {
    let mut cfg = plugin_config(id, &format!("vcs_log_parity_{id}"), lang);
    cfg.ui.commit_meta_align = align.into();
    cfg.plugins.entry(id.into()).or_default().insert(
        "commit_meta_align".into(),
        toml::Value::String(align.into()),
    );
    let (mut old, mut new) = pair_config(root, id, cfg, width, height);
    old.press(KeyCode::Tab, KeyModifiers::NONE);
    new.press(KeyCode::Tab, KeyModifiers::NONE);
    drain(&mut new);
    assert!(old.app.is_git_log());
    assert_eq!(new.app.project_frame().unwrap().key.screen, "log");
    (old, new)
}

fn rows(sim: &Sim) -> Vec<String> {
    let content = sim.app.project_content().unwrap();
    let ProjectContent::List(list) = content.as_ref() else {
        panic!("list required")
    };
    sim.app
        .project_visible_rows()
        .iter()
        .map(|i| list.rows[*i].value.clone())
        .collect()
}

fn same(old: &mut Sim, new: &mut Sim, width: u16, height: u16) {
    let differences = compare_body(old, new, width, height);
    assert!(
        differences.is_empty(),
        "{}",
        differences
            .iter()
            .take(8)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}

fn detail_repo() -> TmpDir {
    let dir = repo(1);
    for (path, text) in [
        ("main.rs", "let value = 123;"),
        ("data.json", "{\"value\":123}"),
    ] {
        let body = (0..70)
            .map(|i| format!("{text} // {i} 日本語 {}\n", "long ".repeat(45)))
            .collect::<String>();
        std::fs::write(dir.join(path), body).unwrap();
    }
    let repository = git2::Repository::open(&dir).unwrap();
    let mut index = repository.index().unwrap();
    index.add_path(std::path::Path::new("main.rs")).unwrap();
    index.add_path(std::path::Path::new("data.json")).unwrap();
    index.write().unwrap();
    let tree_id = index.write_tree().unwrap();
    let tree = repository.find_tree(tree_id).unwrap();
    for (i, message) in [
        "Subject 日本語\n\nBody one\n\nBody three",
        "Empty subject\n\nEmpty body",
    ]
    .into_iter()
    .enumerate()
    {
        let message = if i == 0 {
            format!("{message} {}", "long message ".repeat(45))
        } else {
            message.to_string()
        };
        let sig = git2::Signature::new(
            "作者 Author",
            "fixture@example.invalid",
            &git2::Time::new(1_577_836_860 + i as i64 * 60, 0),
        )
        .unwrap();
        let parent = repository.head().unwrap().peel_to_commit().unwrap();
        repository
            .commit(Some("HEAD"), &sig, &sig, &message, &tree, &[&parent])
            .unwrap();
    }
    dir
}

#[test]
fn e2e_vcs_detail_cells_motions_layout_empty_and_less_match_legacy() {
    let dir = detail_repo();
    for width in [80, 180] {
        for layout in ["unified", "auto"] {
            for empty in [false, true] {
                for scheme in ["vim", "less"] {
                    let height = 14;
                    let mut cfg = plugin_config("git", "vcs_detail_cells", "en");
                    cfg.ui.keys = scheme.into();
                    cfg.git.diff = layout.into();
                    let (mut old, mut new) = pair_config(&dir, "git", cfg, width, height);
                    for sim in [&mut old, &mut new] {
                        sim.press(KeyCode::Tab, KeyModifiers::NONE);
                    }
                    drain(&mut new);
                    if !empty {
                        old.key('j');
                        new.key('j');
                    }
                    let cursor = new.app.project_frame().unwrap().cursor;
                    old.enter();
                    new.enter();
                    drain(&mut new);
                    assert_eq!(new.app.project_frame().unwrap().key.screen, "detail");
                    let content = new.app.project_content().unwrap();
                    let ProjectContent::Diff(diff) = content.as_ref() else {
                        panic!("diff required")
                    };
                    assert_eq!(diff.lines.is_empty(), empty);
                    if !empty {
                        assert_eq!(diff.lines.len(), 142);
                    }
                    same(&mut old, &mut new, width, height);
                    for (code, modifiers) in [
                        (KeyCode::Char('j'), KeyModifiers::NONE),
                        (KeyCode::Char('j'), KeyModifiers::NONE),
                        (KeyCode::Char('j'), KeyModifiers::NONE),
                        (KeyCode::Char('d'), KeyModifiers::CONTROL),
                        (KeyCode::Char('G'), KeyModifiers::NONE),
                        (KeyCode::Char('l'), KeyModifiers::NONE),
                        (KeyCode::Char('l'), KeyModifiers::NONE),
                        (KeyCode::Char('s'), KeyModifiers::NONE),
                        (KeyCode::Char('l'), KeyModifiers::NONE),
                        (KeyCode::Char('0'), KeyModifiers::NONE),
                        (KeyCode::Char('$'), KeyModifiers::NONE),
                        (KeyCode::Char('h'), KeyModifiers::NONE),
                        (KeyCode::Char('g'), KeyModifiers::NONE),
                        (KeyCode::PageDown, KeyModifiers::NONE),
                        (KeyCode::PageUp, KeyModifiers::NONE),
                        (
                            KeyCode::Char('d'),
                            if scheme == "less" {
                                KeyModifiers::NONE
                            } else {
                                KeyModifiers::CONTROL
                            },
                        ),
                        (
                            KeyCode::Char('u'),
                            if scheme == "less" {
                                KeyModifiers::NONE
                            } else {
                                KeyModifiers::CONTROL
                            },
                        ),
                        (
                            KeyCode::Char('f'),
                            if scheme == "less" {
                                KeyModifiers::NONE
                            } else {
                                KeyModifiers::CONTROL
                            },
                        ),
                        (
                            KeyCode::Char('b'),
                            if scheme == "less" {
                                KeyModifiers::NONE
                            } else {
                                KeyModifiers::CONTROL
                            },
                        ),
                    ] {
                        old.press(code, modifiers);
                        new.press(code, modifiers);
                        drain(&mut new);
                        if code == KeyCode::Char('s') {
                            assert_eq!(new.app.flash, old.app.flash);
                            assert_eq!(new.app.project_frame().unwrap().hscroll, 0);
                        }
                        same(&mut old, &mut new, width, height);
                    }
                    for code in [KeyCode::Tab, KeyCode::BackTab] {
                        new.press(code, KeyModifiers::NONE);
                        drain(&mut new);
                        assert_eq!(new.app.project_frame().unwrap().key.screen, "detail");
                    }
                    new.key('q');
                    drain(&mut new);
                    assert_eq!(new.app.project_frame().unwrap().key.screen, "log");
                    assert_eq!(new.app.project_frame().unwrap().cursor, cursor);
                    new.key('l');
                    drain(&mut new);
                    assert_eq!(new.app.project_frame().unwrap().key.screen, "detail");
                    new.press(KeyCode::Esc, KeyModifiers::NONE);
                    drain(&mut new);
                    assert_eq!(new.app.project_frame().unwrap().cursor, cursor);
                }
            }
        }
    }
}

#[test]
fn e2e_vcs_detail_pushed_screen_is_absent_from_tabs_cycle_and_empty_row_open() {
    let dir = repo(0);
    let cfg = plugin_config("git", "vcs_detail_tabs", "en");
    let (_, mut new) = pair_config(&dir, "git", cfg, 100, 12);
    for (code, expected) in [
        (KeyCode::Tab, "log"),
        (KeyCode::Tab, "worktrees"),
        (KeyCode::Tab, "changes"),
        (KeyCode::BackTab, "worktrees"),
        (KeyCode::BackTab, "log"),
    ] {
        new.press(code, KeyModifiers::NONE);
        drain(&mut new);
        assert_eq!(new.app.project_frame().unwrap().key.screen, expected);
        let buffer = new.term.backend().buffer();
        let tabs = (0..100)
            .map(|x| buffer[(x, 1)].symbol())
            .collect::<String>();
        assert!(!tabs.contains("Detail"), "{tabs}");
    }
    assert!(new.app.project_act_context().selected.is_none());
    assert!(!new.app.project_controls().unwrap().contains("/l/"));
    for code in [KeyCode::Enter, KeyCode::Char('l'), KeyCode::Right] {
        new.press(code, KeyModifiers::NONE);
        drain(&mut new);
        assert_eq!(new.app.project_frame().unwrap().key.screen, "log");
    }
}

#[test]
fn e2e_vcs_detail_missing_commit_empty_without_header_matches_legacy_and_r_reloads() {
    for width in [80, 180] {
        for layout in ["unified", "auto"] {
            let dir = detail_repo();
            let mut cfg = plugin_config("git", "vcs_detail_missing", "en");
            cfg.git.diff = layout.into();
            let (mut old, mut new) = pair_config(&dir, "git", cfg, width, 12);
            old.press(KeyCode::Tab, KeyModifiers::NONE);
            new.press(KeyCode::Tab, KeyModifiers::NONE);
            drain(&mut new);
            let id = new.app.project_act_context().selected.unwrap().value;
            old.enter();
            new.enter();
            drain(&mut new);
            let before = new.app.project_content().unwrap();
            std::fs::remove_file(dir.join(".git/objects").join(&id[..2]).join(&id[2..])).unwrap();
            old.app.close_git_detail();
            old.enter();
            new.key('r');
            drain(&mut new);
            let after = new.app.project_content().unwrap();
            assert!(!std::sync::Arc::ptr_eq(&before, &after));
            let ProjectContent::Diff(diff) = after.as_ref() else {
                panic!("diff required")
            };
            assert!(diff.lines.is_empty() && diff.header.is_empty());
            same(&mut old, &mut new, width, 12);
            for (code, modifiers) in [
                (KeyCode::Char('j'), KeyModifiers::NONE),
                (KeyCode::Char('j'), KeyModifiers::NONE),
                (KeyCode::Char('j'), KeyModifiers::NONE),
                (KeyCode::Char('d'), KeyModifiers::CONTROL),
                (KeyCode::Char('G'), KeyModifiers::NONE),
                (KeyCode::Char('l'), KeyModifiers::NONE),
                (KeyCode::Char('l'), KeyModifiers::NONE),
                (KeyCode::Char('s'), KeyModifiers::NONE),
                (KeyCode::Char('l'), KeyModifiers::NONE),
                (KeyCode::Char('0'), KeyModifiers::NONE),
            ] {
                old.press(code, modifiers);
                new.press(code, modifiers);
                drain(&mut new);
                same(&mut old, &mut new, width, 12);
            }
        }
    }
}

#[test]
fn e2e_vcs_project_list_right_opens_changes_like_enter_and_respects_remaps() {
    let dir = detail_repo();
    std::fs::write(dir.join("main.rs"), "changed\n").unwrap();
    for (code, remap) in [
        (KeyCode::Char('l'), false),
        (KeyCode::Right, false),
        (KeyCode::Char('Z'), true),
    ] {
        let mut cfg = plugin_config("git", "vcs_list_right", "en");
        if remap {
            cfg.keys.surfaces.insert(
                "text".into(),
                [
                    ("Z".into(), "navigate:right".into()),
                    ("l".into(), "noop".into()),
                ]
                .into(),
            );
        }
        let (_, mut new) = pair_config(&dir, "git", cfg, 100, 12);
        if remap {
            new.key('l');
            drain(&mut new);
            assert!(new.app.project_active());
        }
        new.press(code, KeyModifiers::NONE);
        drain(&mut new);
        assert!(new.app.is_git_diff_preview(), "code={code:?}");
    }
}

#[test]
fn e2e_vcs_detail_layout_remap_uses_diff_context_for_detail_changes_and_file() {
    let dir = detail_repo();
    std::fs::write(dir.join("main.rs"), "changed\n").unwrap();
    for table in ["git.detail", "text"] {
        let mut cfg = plugin_config("git", "vcs_detail_layout_remap", "en");
        cfg.keys.surfaces.insert(
            "diff".into(),
            [
                ("S".into(), "cycle_diff_layout".into()),
                ("s".into(), "noop".into()),
            ]
            .into(),
        );
        cfg.keys.surfaces.insert(
            table.into(),
            [
                ("S".into(), "noop".into()),
                ("s".into(), "cycle_diff_layout".into()),
            ]
            .into(),
        );
        let (mut old, mut new) = pair_config(&dir, "git", cfg, 80, 12);
        old.press(KeyCode::Tab, KeyModifiers::NONE);
        new.press(KeyCode::Tab, KeyModifiers::NONE);
        drain(&mut new);
        old.enter();
        new.enter();
        drain(&mut new);
        new.key('s');
        drain(&mut new);
        assert!(!new.app.diff_is_split(78), "{table}");
        new.key('S');
        drain(&mut new);
        assert!(new.app.diff_is_split(78), "{table}");
        assert!(new.app.project_controls().unwrap().contains("S:"));
        assert!(crate::ops::help::sections(&new.app)
            .iter()
            .flat_map(|s| &s.rows)
            .any(|(key, _)| key == "S"));
        new.key('q');
        drain(&mut new);
        new.press(KeyCode::BackTab, KeyModifiers::NONE);
        drain(&mut new);
        new.enter();
        drain(&mut new);
        assert!(new.app.is_git_diff_preview() && !new.app.project_active());
        old.key('q');
        old.key('q');
        old.app.open_git_diff(&dir.join("main.rs"));
        old.draw();
        for sim in [&mut old, &mut new] {
            assert_eq!(sim.app.key_context(), Some(crate::keymap::Context::Diff));
            let before = sim.app.diff_is_split(78);
            sim.key('s');
            assert_eq!(sim.app.diff_is_split(78), before);
            sim.key('S');
            assert_ne!(sim.app.diff_is_split(78), before);
            assert!(
                crate::ops::footer::tokens(&sim.app, crate::keymap::Context::Diff)
                    .iter()
                    .any(|token| token.starts_with("S:"))
            );
            assert!(crate::ops::help::sections(&sim.app)
                .iter()
                .flat_map(|s| &s.rows)
                .any(|(key, _)| key == "S"));
        }
    }
}

#[test]
fn e2e_vcs_log_git_cells_alignments_boundaries_cjk_ellipsis_and_scrolling() {
    use unicode_width::UnicodeWidthStr;
    let dir = repo(13);
    let latest = &crate::git::log(&dir, 200)[0];
    let meta = format!("{} · 2020-01-01 00:12", latest.author);
    let mw = meta.width() as u16;
    for lang in ["en", "ja"] {
        for align in ["right", "inline", "unknown"] {
            // Terminal width includes two border columns; mw+2 is the exact fit threshold.
            for width in [mw + 3, mw + 4, 12, 58, 220] {
                let height = 8;
                let (mut old, mut new) = pair(&dir, "git", lang, align, width, height);
                same(&mut old, &mut new, width, height);
                for code in [
                    KeyCode::Char('j'),
                    KeyCode::Char('G'),
                    KeyCode::Char('k'),
                    KeyCode::Char('g'),
                    KeyCode::PageDown,
                    KeyCode::PageUp,
                    KeyCode::End,
                    KeyCode::Home,
                    KeyCode::Down,
                    KeyCode::Up,
                ] {
                    // Legacy paging has no PageUp/PageDown binding; compare at the same cursor.
                    match code {
                        KeyCode::PageDown => old.app.git_log_move(10),
                        KeyCode::PageUp => old.app.git_log_move(-10),
                        KeyCode::Home => old.app.git_log_move(-1000),
                        KeyCode::End => old.app.git_log_move(1000),
                        _ => old.press(code, KeyModifiers::NONE),
                    }
                    new.press(code, KeyModifiers::NONE);
                    drain(&mut new);
                    assert_eq!(
                        new.app.project_frame().unwrap().cursor,
                        old.app.git_log_sel()
                    );
                    same(&mut old, &mut new, width, height);
                }
            }
        }
    }
}

#[test]
fn e2e_vcs_log_filter_uses_legacy_text_and_cursor_for_every_query() {
    let dir = repo(13);
    let short = crate::git::log(&dir, 1)[0].short.clone();
    for align in ["right", "inline"] {
        for query in [
            "長い件名",
            "作者",
            "Author",
            "2020-01-01 00:12",
            short.as_str(),
            "Summary 作者",
            "·",
            "SUMMARY",
            "summary",
        ] {
            let (mut old, mut new) = pair(&dir, "git", "en", align, 100, 8);
            for sim in [&mut old, &mut new] {
                sim.key('/');
                for c in query.chars() {
                    sim.key(c);
                }
                sim.enter();
            }
            drain(&mut new);
            let expected = old
                .app
                .list_visible(ListId::Log)
                .unwrap()
                .iter()
                .map(|i| old.app.git_log_entries()[*i].id.clone())
                .collect::<Vec<_>>();
            assert_eq!(rows(&new), expected, "query={query}");
            let expected_cursor = expected
                .iter()
                .position(|id| Some(id.clone()) == old.app.git_log_selected_id())
                .unwrap_or(0);
            assert_eq!(
                new.app.project_frame().unwrap().cursor,
                expected_cursor,
                "query={query}"
            );
            same(&mut old, &mut new, 100, 8);
            old.key('j');
            new.key('j');
            drain(&mut new);
            same(&mut old, &mut new, 100, 8);
        }
    }
}

#[test]
fn e2e_vcs_log_r_refreshes_current_screen_and_remaps_filtered_identity() {
    let dir = repo(3);
    let (mut old, mut new) = pair(&dir, "git", "en", "right", 100, 8);
    for sim in [&mut old, &mut new] {
        sim.key('/');
        for c in "Summary 作者".chars() {
            sim.key(c);
        }
        sim.enter();
    }
    let selected = new.app.project_act_context().selected.unwrap().value;
    commit(&dir, 3);
    assert_eq!(
        rows(&new).len(),
        3,
        "injected watcher must not refresh spontaneously"
    );
    old.key('r');
    new.key('r');
    drain(&mut new);
    assert_eq!(rows(&new).len(), 4);
    assert_eq!(
        new.app.project_act_context().selected.unwrap().value,
        selected
    );
    assert_eq!(new.app.project_frame().unwrap().cursor, 1);
    assert_eq!(old.app.git_log_sel(), 0);
    assert_ne!(old.app.git_log_selected_id(), Some(selected));
    // r applies to every project screen, including the original changes screen.
    new.press(KeyCode::BackTab, KeyModifiers::NONE);
    drain(&mut new);
    std::fs::write(dir.join("added"), "new\n").unwrap();
    new.key('r');
    drain(&mut new);
    new.see("added");
}

#[test]
fn e2e_vcs_log_head_and_refs_injected_watch_events_refresh_active_log() {
    let dir = repo(3);
    let (_, mut new) = pair(&dir, "git", "en", "right", 100, 8);
    for (i, path) in [(3, ".git/HEAD"), (4, ".git/refs/heads/main")] {
        commit(&dir, i);
        assert_eq!(rows(&new).len(), i);
        new.app
            .tab
            .projects
            .inject_event(Ok(notify::Event::new(notify::EventKind::Modify(
                notify::event::ModifyKind::Any,
            ))
            .add_path(dir.join(path))));
        drain(&mut new);
        assert_eq!(rows(&new).len(), i + 1);
    }
}

#[test]
fn e2e_vcs_log_unborn_empty_cells_and_fixed_tab_order() {
    for lang in ["en", "ja"] {
        let dir = repo(1);
        let cfg = plugin_config("git", "vcs_log_empty", lang);
        // Remove the only ref after opening changes, making HEAD unborn before cycling.
        let mut old = Sim::with_config_sized(&dir, cfg.clone(), 80, 8);
        old.app.open_git_view();
        let repository = git2::Repository::open(&dir).unwrap();
        let head = repository.head().unwrap().name().unwrap().to_string();
        repository.find_reference(&head).unwrap().delete().unwrap();
        old.press(KeyCode::Tab, KeyModifiers::NONE);
        assert!(!old.app.is_git_log());
        assert!(old.app.flash.is_some());
        old.app.show_empty_git_log_for_test();
        let mut new = Sim::with_config_sized(&dir, cfg, 80, 8);
        new.app.tab.projects.wait_for_result();
        new.app.poll_projects();
        new.key('p');
        drain(&mut new);
        new.press(KeyCode::Tab, KeyModifiers::NONE);
        drain(&mut new);
        assert_eq!(new.app.project_frame().unwrap().key.screen, "log");
        assert!(new.app.project_act_context().selected.is_none());
        same(&mut old, &mut new, 80, 8);
    }
}

#[test]
fn e2e_vcs_log_exit_returns_to_origin_and_h_does_nothing() {
    let dir = repo(3);
    for code in [KeyCode::Char('q'), KeyCode::Esc] {
        let (mut old, mut new) = pair(&dir, "git", "en", "right", 80, 8);
        {
            new.press(KeyCode::Char('h'), KeyModifiers::NONE);
            drain(&mut new);
            assert_eq!(new.app.project_frame().unwrap().key.screen, "log");
            assert!(new.app.pending_leader.is_none());
        }
        old.press(code, KeyModifiers::NONE);
        new.press(code, KeyModifiers::NONE);
        drain(&mut new);
        assert!(old.app.is_git_view());
        assert!(!new.app.project_active());
    }
}

#[test]
fn e2e_vcs_log_limit_matches_hub_at_200_and_omits_201st() {
    let dir = repo(205);
    let (mut old, mut new) = pair(&dir, "git", "en", "right", 100, 8);
    assert_eq!(rows(&new).len(), 200);
    assert_eq!(
        rows(&new),
        old.app
            .git_log_entries()
            .iter()
            .map(|c| c.id.clone())
            .collect::<Vec<_>>()
    );
    old.key('G');
    new.key('G');
    drain(&mut new);
    same(&mut old, &mut new, 100, 8);
}

fn jj(root: &std::path::Path, args: &[&str]) {
    let output = std::process::Command::new("jj")
        .current_dir(root)
        .env("HOME", root)
        .env("JJ_USER", "作者 Author")
        .env("JJ_EMAIL", "fixture@example.invalid")
        .args([
            "--config",
            "debug.commit-timestamp=\"2020-01-01T00:20:00Z\"",
            "--config",
            "debug.operation-timestamp=\"2020-01-01T00:20:00Z\"",
        ])
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "jj {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

struct Preference;
impl Preference {
    fn pin(id: &str) -> Self {
        crate::vcs::set_preference_for_test(Some(if id == "git" {
            crate::vcs::Preference::Git
        } else {
            crate::vcs::Preference::Jj
        }));
        Self
    }
}
impl Drop for Preference {
    fn drop(&mut self) {
        crate::vcs::set_preference_for_test(None);
    }
}

#[test]
fn e2e_vcs_log_jj_and_both_colocated_backends_match_the_selected_legacy_backend() {
    if !crate::vcs::jj::available() {
        return;
    }
    for colocated in [false, true] {
        let dir = if colocated {
            repo(5)
        } else {
            let dir = crate::test_support::unique_tmp("vcs_log_jj");
            std::fs::create_dir_all(&dir).unwrap();
            dir.into_canonical().unwrap()
        };
        jj(
            &dir,
            &[
                "git",
                "init",
                if colocated {
                    "--colocate"
                } else {
                    "--no-colocate"
                },
                ".",
            ],
        );
        jj(&dir, &["describe", "-m", "jj 専用 Summary"]);
        jj(&dir, &["new", "-m", "次の日本語 Summary"]);
        let commits = konoma_vcs_core::vcs::jj::log(&dir, 200);
        assert!(commits.iter().any(|c| c.summary == "jj 専用 Summary"));
        assert!(commits
            .iter()
            .filter(|c| c.summary.contains("Summary") && !c.summary.contains("日本語の長い"))
            .any(|c| c.time_epoch == 1_577_838_000));
        for id in if colocated {
            vec!["git", "jj"]
        } else {
            vec!["jj"]
        } {
            let _preference = Preference::pin(id);
            if id == "jj" {
                let (mut old, mut new) = pair(&dir, id, "en", "right", 80, 12);
                old.enter();
                new.enter();
                drain(&mut new);
                let content = new.app.project_content().unwrap();
                let ProjectContent::Diff(diff) = content.as_ref() else {
                    panic!("diff required")
                };
                assert!(diff.header[0].spans[0].content.starts_with("change "));
                same(&mut old, &mut new, 80, 12);
            }
            for align in ["right", "inline", "unknown"] {
                for lang in ["en", "ja"] {
                    let (mut old, mut new) = pair(&dir, id, lang, align, 72, 7);
                    assert_eq!(
                        rows(&new),
                        old.app
                            .git_log_entries()
                            .iter()
                            .map(|c| c.id.clone())
                            .collect::<Vec<_>>()
                    );
                    for _ in 0..old.app.git_log_entries().len() {
                        same(&mut old, &mut new, 72, 7);
                        old.key('j');
                        new.key('j');
                        drain(&mut new);
                    }
                }
            }
        }
        let _preference = Preference::pin("jj");
        let (_, mut new) = pair(&dir, "jj", "en", "right", 80, 8);
        let before = rows(&new);
        jj(&dir, &["new", "-m", "watch new"]);
        new.app
            .tab
            .projects
            .inject_event(Ok(notify::Event::new(notify::EventKind::Modify(
                notify::event::ModifyKind::Any,
            ))
            .add_path(dir.join(".jj/repo/op_heads"))));
        drain(&mut new);
        assert_ne!(rows(&new), before);
        assert!(rows(&new).contains(&konoma_vcs_core::vcs::jj::log(&dir, 1)[0].id));
    }
}
