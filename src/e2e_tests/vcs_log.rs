//! Compare linear log bodies through the legacy hub and the project host.

use super::vcs_changes::{compare_body, drain, pair_config, plugin_config};
use super::*;
use crate::app::list_filter::ListId;
use crate::app::project_screens::ProjectContent;
use crate::test_support::{init_git_repo, TmpDir};
use konoma_vcs_core::git2;

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
fn e2e_vcs_log_exit_returns_to_origin_and_unimplemented_keys_do_nothing() {
    let dir = repo(3);
    for code in [KeyCode::Char('q'), KeyCode::Esc] {
        let (mut old, mut new) = pair(&dir, "git", "en", "right", 80, 8);
        for key in [KeyCode::Enter, KeyCode::Char('l'), KeyCode::Char('h')] {
            new.press(key, KeyModifiers::NONE);
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
