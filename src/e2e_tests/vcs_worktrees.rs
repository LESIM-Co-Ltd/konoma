//! Linked worktrees through the legacy and project hosts.

use super::super::vcs_changes::{compare_body, drain, pair_config, plugin_config};
use super::super::*;
use crate::app::project_screens::ProjectContent;
use crate::test_support::TmpDir;
use konoma_vcs_core::git2;
use std::path::{Path, PathBuf};

fn git(root: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .current_dir(root)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

fn commit(root: &Path, message: &str, time: i64) {
    let repo = git2::Repository::open(root).unwrap();
    let mut index = repo.index().unwrap();
    index
        .add_all(["*"], git2::IndexAddOption::DEFAULT, None)
        .unwrap();
    index.write().unwrap();
    let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
    let signature = git2::Signature::new(
        "Fixture",
        "fixture@example.invalid",
        &git2::Time::new(time, 0),
    )
    .unwrap();
    let parent = repo.head().ok().map(|h| h.peel_to_commit().unwrap());
    repo.commit(
        Some("HEAD"),
        &signature,
        &signature,
        message,
        &tree,
        &parent.iter().collect::<Vec<_>>(),
    )
    .unwrap();
}

struct Repo {
    root: PathBuf,
    linked: PathBuf,
    detached: PathBuf,
    _dir: TmpDir,
}

fn repo() -> Repo {
    let dir = sandbox("vcs_worktrees").into_canonical().unwrap();
    let root = dir.join("main");
    git2::Repository::init(&root).unwrap();
    git(&root, &["branch", "-m", "main"]);
    std::fs::write(root.join("file.txt"), "base\n").unwrap();
    commit(&root, "seed", 1_577_836_800);
    let linked = dir.join("linked");
    let detached = dir.join("detached");
    git(
        &root,
        &["worktree", "add", "-qb", "日本語", linked.to_str().unwrap()],
    );
    git(
        &root,
        &[
            "worktree",
            "add",
            "-q",
            "--detach",
            detached.to_str().unwrap(),
        ],
    );
    let missing = dir.join("prunable");
    git(
        &root,
        &[
            "worktree",
            "add",
            "-qb",
            "missing",
            missing.to_str().unwrap(),
        ],
    );
    std::fs::remove_dir_all(&missing).unwrap();
    git(
        &root,
        &[
            "worktree",
            "lock",
            "--reason",
            "fixture lock",
            linked.to_str().unwrap(),
        ],
    );
    Repo {
        root,
        linked,
        detached,
        _dir: dir,
    }
}

fn pair(root: &Path, lang: &str, width: u16) -> (Sim, Sim) {
    pair_settings(root, lang, width, "../", false, &[])
}

fn pair_settings(
    root: &Path,
    lang: &str,
    width: u16,
    worktree_dir: &str,
    read_only: bool,
    bases: &[&str],
) -> (Sim, Sim) {
    let mut cfg = plugin_config("git", "vcs_worktrees", lang);
    cfg.git.worktree_dir = worktree_dir.into();
    cfg.ui.graph_base_branches = bases.iter().map(|s| (*s).into()).collect();
    let settings = cfg.plugins.entry("git".into()).or_default();
    settings.insert(
        "worktree_dir".into(),
        toml::Value::String(worktree_dir.into()),
    );
    settings.insert("read_only".into(), toml::Value::Boolean(read_only));
    settings.insert(
        "graph_base_branches".into(),
        toml::Value::Array(
            bases
                .iter()
                .map(|s| toml::Value::String((*s).into()))
                .collect(),
        ),
    );
    let (mut old, mut new) = pair_config(root, "git", cfg, width, 20);
    old.key('w');
    new.key('w');
    drain(&mut new);
    assert!(old.app.is_git_worktrees());
    assert_eq!(new.app.project_frame().unwrap().key.screen, "worktrees");
    (old, new)
}

fn submit(old: &mut Sim, new: &mut Sim, name: &str) {
    old.key('c');
    new.key('c');
    let (_, title, buffer, _) = old.app.dialog_view().unwrap();
    assert!(buffer.is_empty());
    let Some(crate::app::project_screens::ProjectDialog::Act {
        prompts, buffer, ..
    }) = &new.app.project_dialog
    else {
        panic!("text dialog required")
    };
    let crate::app::project_screens::ProjectPrompt::Text(prompt) = &prompts[0] else {
        panic!("text input required")
    };
    assert_eq!(prompt.label.get(new.app.plugin_lang()), title);
    assert!(buffer.is_empty());
    old.keys(name);
    new.keys(name);
    old.enter();
    new.enter();
    drain(new);
}

#[test]
fn e2e_vcs_worktrees_create_dialog_blank_and_git_failure_flash_without_reopen() {
    let repo = repo();
    for lang in ["en", "ja"] {
        for name in ["   ", "main"] {
            let (mut old, mut new) = pair(&repo.root, lang, 160);
            submit(&mut old, &mut new, name);
            assert_eq!(new.app.flash, old.app.flash);
            assert!(!old.app.is_dialog());
            assert!(new.app.project_dialog.is_none());
            assert_eq!(new.app.tab.root, repo.root);
            assert_eq!(new.app.project_frame().unwrap().key.screen, "worktrees");
        }
    }
}

#[test]
fn e2e_vcs_worktrees_create_switches_to_legacy_target_with_slash_existing_branch_and_custom_directory(
) {
    for lang in ["en", "ja"] {
        for (name, directory, existing) in [
            ("  feat/nested  ", "../", false),
            ("existing", "../placed", true),
        ] {
            let repo = repo();
            if existing {
                git(&repo.root, &["branch", name]);
            }
            let (mut old, mut new) = pair_settings(&repo.linked, lang, 160, directory, false, &[]);
            // Run the two writes against the same target, restoring only this fixture between them.
            old.key('c');
            old.keys(name);
            old.enter();
            assert!(!old.app.is_dialog());
            let target = old.app.tab.root.clone();
            let open_dir = old.app.tab.open_dir.clone();
            let flash = old.app.flash.clone();
            assert_eq!(
                target.file_name().unwrap(),
                name.trim().replace('/', "-").as_str()
            );
            assert_eq!(
                target,
                repo.root
                    .join(directory)
                    .join(name.trim().replace('/', "-"))
                    .canonicalize()
                    .unwrap()
            );
            drop(old);
            git(
                &repo.root,
                &["worktree", "remove", "--force", target.to_str().unwrap()],
            );
            if !existing {
                git(&repo.root, &["branch", "-D", name.trim()]);
            }
            new.key('c');
            new.keys(name);
            new.enter();
            drain(&mut new);
            assert_eq!(new.app.tab.root, target);
            assert_eq!(new.app.tab.open_dir, open_dir);
            assert_eq!(new.app.flash, flash);
            assert!(new.app.project_dialog.is_none());
            assert!(!new.app.project_active());
            assert!(target.join(".git").is_file());
        }
    }
}

fn same(old: &mut Sim, new: &mut Sim, width: u16) {
    let differences = compare_body(old, new, width, 20);
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

fn same_diff(old: &mut Sim, new: &mut Sim, width: u16) {
    let before = old
        .term
        .draw(|frame| ui::render(frame, &mut old.app))
        .unwrap()
        .buffer
        .clone();
    let after = new
        .term
        .draw(|frame| ui::render(frame, &mut new.app))
        .unwrap()
        .buffer
        .clone();
    // The legacy title is in the frame; the new diff header and divider occupy two body rows.
    for y in 2..16 {
        for x in 0..width - 2 {
            let a = &before[(x + 1, y)];
            let b = &after[(x, y + 2)];
            assert_eq!(
                (a.symbol(), a.fg, a.bg, a.modifier),
                (b.symbol(), b.fg, b.bg, b.modifier),
                "({x}, {y})"
            );
        }
    }
}

fn select(old: &mut Sim, new: &mut Sim, path: &Path) {
    let index = old
        .app
        .git_worktree_view()
        .iter()
        .position(|w| w.path == path)
        .unwrap();
    while old.app.git_worktree_sel() != index {
        old.key(if old.app.git_worktree_sel() < index {
            'j'
        } else {
            'k'
        });
    }
    while new.app.project_frame().unwrap().cursor != index {
        new.key(if new.app.project_frame().unwrap().cursor < index {
            'j'
        } else {
            'k'
        });
        drain(new);
    }
}

#[test]
fn e2e_vcs_worktrees_enter_each_kind_matches_legacy_root_open_dir_and_flash_including_l() {
    let repo = repo();
    git(&repo._dir, &["clone", "-q", "--bare", "main", "bare"]);
    let bare = repo._dir.join("bare");
    let bare_linked = repo._dir.join("bare-linked");
    git(
        &bare,
        &[
            "worktree",
            "add",
            "-q",
            "--detach",
            bare_linked.to_str().unwrap(),
        ],
    );
    for lang in ["en", "ja"] {
        for (root, path, kind) in [
            (&repo.root, repo.linked.as_path(), "worktree"),
            (&repo.root, repo.detached.as_path(), "worktree"),
            (&repo.root, repo.root.as_path(), "current"),
            (
                &repo.root,
                repo._dir.join("prunable").as_path(),
                "unavailable",
            ),
            (&bare_linked, bare.as_path(), "bare"),
        ] {
            for code in [KeyCode::Enter, KeyCode::Char('l')] {
                let (mut old, mut new) = pair(root, lang, 160);
                select(&mut old, &mut new, path);
                old.press(code, KeyModifiers::NONE);
                new.press(code, KeyModifiers::NONE);
                drain(&mut new);
                assert_eq!(new.app.tab.root, old.app.tab.root, "{kind}");
                assert_eq!(new.app.tab.open_dir, old.app.tab.open_dir, "{kind}");
                assert_eq!(new.app.flash, old.app.flash, "{kind}");
                if kind != "worktree" {
                    assert_eq!(&new.app.tab.root, root);
                    assert!(new.app.project_active());
                    assert_eq!(new.app.project_frame().unwrap().key.screen, "worktrees");
                }
            }
        }
    }
}

#[test]
fn e2e_vcs_worktrees_ctrl_t_directory_root_open_dir_preserves_original_and_file_target_still_previews(
) {
    let repo = repo();
    let (mut old, mut new) = pair(&repo.root, "en", 160);
    select(&mut old, &mut new, &repo.linked);
    let cursor = new.app.project_frame().unwrap().cursor;
    old.press(KeyCode::Char('t'), KeyModifiers::CONTROL);
    new.press(KeyCode::Char('t'), KeyModifiers::CONTROL);
    drain(&mut new);
    assert_eq!(new.app.tab_count(), 2);
    assert_eq!(new.app.tab.root, old.app.tab.root);
    assert_eq!(new.app.tab.open_dir, old.app.tab.open_dir);
    assert_eq!(new.app.tab.root, repo.linked);
    assert_eq!(new.app.tab.mode, Mode::Tree);
    assert!(!new.app.tab.entries.is_empty());
    new.app.tab_goto(0);
    drain(&mut new);
    assert_eq!(new.app.tab.root, repo.root);
    assert_eq!(new.app.project_frame().unwrap().key.screen, "worktrees");
    assert_eq!(new.app.project_frame().unwrap().cursor, cursor);
    std::fs::write(repo.root.join("file.txt"), "changed\n").unwrap();
    let cfg = plugin_config("git", "vcs_worktrees_file_tab", "en");
    let (_, mut file) = pair_config(&repo.root, "git", cfg, 160, 20);
    file.press(KeyCode::Char('t'), KeyModifiers::CONTROL);
    drain(&mut file);
    assert_eq!(file.app.tab_count(), 2);
    assert_eq!(file.app.tab.mode, Mode::Preview);
    assert_eq!(
        file.app.tab.preview_path.as_deref(),
        Some(repo.root.join("file.txt").as_path())
    );
}

#[test]
fn e2e_vcs_worktree_filter_ignores_control_chords_like_legacy_and_keeps_shift_text() {
    use crate::app::list_filter::ListId;
    let repo = repo();
    for lang in ["en", "ja"] {
        let (mut old, mut new) = pair(&repo.root, lang, 160);
        select(&mut old, &mut new, &repo.linked);
        old.key('/');
        new.key('/');
        for letter in ['t', 'c', 'x', 'w', 'q', 'a', 'd'] {
            old.press(KeyCode::Char(letter), KeyModifiers::CONTROL);
            new.press(KeyCode::Char(letter), KeyModifiers::CONTROL);
            drain(&mut new);
            assert_eq!(old.app.list_filter_query(ListId::Worktrees), "");
            assert_eq!(
                new.app.project_frame().unwrap().query,
                "",
                "Ctrl-{letter} is not text"
            );
            assert!(new.app.project_frame().unwrap().filtering);
            assert_eq!(new.app.project_frame().unwrap().key.screen, "worktrees");
            assert_eq!(new.app.tab_count(), 1);
            assert_eq!(new.app.tab.root, repo.root);
            assert!(new.app.project_dialog.is_none());
        }
        for (code, mods) in [
            (KeyCode::Char('日'), KeyModifiers::NONE),
            (KeyCode::Char('Q'), KeyModifiers::SHIFT),
        ] {
            old.press(code, mods);
            new.press(code, mods);
        }
        assert_eq!(old.app.list_filter_query(ListId::Worktrees), "日Q");
        assert_eq!(new.app.project_frame().unwrap().query, "日Q");
        for (code, mods) in [
            (KeyCode::Char('x'), KeyModifiers::ALT),
            (KeyCode::Char('\n'), KeyModifiers::NONE),
            (KeyCode::Char('\0'), KeyModifiers::NONE),
        ] {
            new.press(code, mods);
            assert_eq!(new.app.project_frame().unwrap().query, "日Q");
        }
        new.press(KeyCode::Backspace, KeyModifiers::NONE);
        assert_eq!(new.app.project_frame().unwrap().query, "日");
        new.enter();
        assert!(!new.app.project_frame().unwrap().filtering);
        assert_eq!(new.app.project_frame().unwrap().query, "日");
        new.key('/');
        new.esc();
        assert!(new.app.project_frame().unwrap().query.is_empty());
        assert!(!new.app.project_frame().unwrap().filtering);
    }
}

#[test]
fn e2e_vcs_worktree_switch_vanished_after_plugin_check_keeps_screen_and_root() {
    use crate::plugins::{
        test_plugins,
        test_projects::{ObservedVcs, VcsCalls},
    };
    use std::sync::{Arc, Mutex};
    for lang in ["en", "ja"] {
        let repo = repo();
        let calls = Arc::new(Mutex::new(VcsCalls {
            remove_switch_target: true,
            ..Default::default()
        }));
        let token = test_plugins::register(
            &format!("vcs_switch_vanished_{lang}"),
            Arc::new(ObservedVcs {
                plugin: Arc::new(konoma_vcs::Git),
                calls: calls.clone(),
            }),
        );
        let mut cfg = test_plugins::marker_config(&token);
        cfg.ui.lang = lang.into();
        let (mut old, mut new) = pair_config(&repo.root, "git", cfg, 160, 20);
        old.key('w');
        new.key('w');
        drain(&mut new);
        select(&mut old, &mut new, &repo.linked);
        let root = new.app.tab.root.clone();
        let open_dir = new.app.tab.open_dir.clone();
        let frame = new.app.project_frame().unwrap().clone();
        new.enter();
        drain(&mut new);
        assert!(!repo.linked.exists());
        assert!(calls
            .lock()
            .unwrap()
            .acts
            .iter()
            .any(|a| a == "worktree_switch"));
        assert_eq!(new.app.tab.root, root);
        assert_eq!(new.app.tab.open_dir, open_dir);
        assert!(
            new.app.project_active(),
            "a refused root switch must keep the worktree screen"
        );
        let after = new.app.project_frame().unwrap();
        assert_eq!(after.key, frame.key);
        assert_eq!(
            (after.cursor, after.scroll, after.query.as_str()),
            (frame.cursor, frame.scroll, frame.query.as_str())
        );
        assert_eq!(
            new.app.flash,
            Some(format!(
                "{}{}",
                crate::i18n::tr(new.app.lang, crate::i18n::Msg::NotFound),
                repo.linked.display()
            ))
        );
    }
}

#[test]
fn e2e_vcs_worktrees_ctrl_t_without_target_does_nothing_and_has_no_help_or_footer_hint() {
    let repo = repo();
    git(&repo._dir, &["clone", "-q", "--bare", "main", "bare"]);
    let bare = repo._dir.join("bare");
    let bare_linked = repo._dir.join("bare-linked");
    git(
        &bare,
        &[
            "worktree",
            "add",
            "-q",
            "--detach",
            bare_linked.to_str().unwrap(),
        ],
    );
    for (root, path) in [
        (&repo.root, repo.root.clone()),
        (&repo.root, repo._dir.join("prunable")),
        (&bare_linked, bare),
    ] {
        let (mut old, mut new) = pair(root, "en", 160);
        select(&mut old, &mut new, &path);
        let cursor = new.app.project_frame().unwrap().cursor;
        assert!(!new.app.project_controls().unwrap().contains("Ctrl-t"));
        assert!(!crate::ops::help::sections(&new.app)
            .iter()
            .flat_map(|s| &s.rows)
            .any(|(key, _)| key.contains("Ctrl-t")));
        let flash = new.app.flash.clone();
        new.press(KeyCode::Char('t'), KeyModifiers::CONTROL);
        drain(&mut new);
        assert_eq!(new.app.tab_count(), 1);
        assert_eq!(&new.app.tab.root, root);
        assert_eq!(new.app.project_frame().unwrap().cursor, cursor);
        assert_eq!(new.app.flash, flash);
    }
}

#[test]
fn e2e_vcs_worktrees_footer_preserves_legacy_action_priority_and_labels() {
    use crate::keymap::{Backend, Context, Screen};
    let repo = repo();
    for lang in ["en", "ja"] {
        let (mut old, mut new) = pair(&repo.root, lang, 160);
        select(&mut old, &mut new, &repo.linked);
        let operations = |tokens: Vec<String>| -> Vec<String> {
            tokens
                .into_iter()
                .flat_map(|token| token.split("  ").map(str::to_owned).collect::<Vec<_>>())
                .filter_map(|token| {
                    let (key, label) = token.split_once(':')?;
                    if key == "c" || key == "d" || key == "/" {
                        Some(token)
                    } else if key == "Ctrl-t" || key == "C-t" {
                        Some(format!("Ctrl-t:{label}"))
                    } else if key.contains("Enter") || key.contains('↵') {
                        Some(format!("Enter:{label}"))
                    } else {
                        None
                    }
                })
                .collect()
        };
        let old_tokens =
            crate::ops::footer::tokens(&old.app, Context::Screen(Backend::Git, Screen::Worktrees));
        let mut expected = operations(old_tokens.clone());
        assert_eq!(expected.len(), 5, "{old_tokens:?}");
        let filter = expected.pop().unwrap();
        expected.insert(0, filter);
        // Shared open/new-tab hints keep the same position on every project screen.
        let target = expected.remove(3);
        expected.insert(2, target);
        let actual = operations(crate::ui::status::hint_tokens(&new.app));
        assert_eq!(actual, expected);
    }
}

#[test]
fn e2e_vcs_project_footer_log_changes_and_worktrees_have_one_entry_per_key() {
    let repo = repo();
    std::fs::write(repo.root.join("file.txt"), "changed\n").unwrap();
    let cfg = plugin_config("git", "vcs_footer_unique_keys", "en");
    let (_, mut new) = pair_config(&repo.root, "git", cfg, 160, 20);
    for screen in ["changes", "log", "graph", "branches", "worktrees"] {
        if screen != "changes" {
            new.press(KeyCode::Tab, KeyModifiers::NONE);
            drain(&mut new);
        }
        assert_eq!(new.app.project_frame().unwrap().key.screen, screen);
        let tokens = crate::ui::status::hint_tokens(&new.app);
        let entries: Vec<_> = tokens.iter().flat_map(|t| t.split("  ")).collect();
        let enter: Vec<_> = entries
            .iter()
            .filter(|t| {
                t.split_once(':')
                    .is_some_and(|(keys, _)| keys.contains("Enter") || keys.contains('↵'))
            })
            .collect();
        assert_eq!(enter.len(), 1, "{screen}: {entries:?}");
        let mut keys = std::collections::BTreeSet::new();
        for entry in entries {
            let Some((chords, _)) = entry.split_once(':') else {
                continue;
            };
            let chords: Vec<_> = if chords == "/" {
                vec!["/"]
            } else {
                chords.split('/').collect()
            };
            for key in chords {
                assert!(keys.insert(key), "{screen}: duplicate {key} in {tokens:?}");
            }
        }
        if screen == "changes" {
            // The existing shared controls and write order stay in place.
            assert!(tokens[0].starts_with("Tab/Shift-Tab:screens  j/k:move"));
            let writes: Vec<_> = tokens
                .iter()
                .filter(|t| {
                    ["s/S:", "u/U:", "x:", "c:"]
                        .iter()
                        .any(|key| t.starts_with(key))
                })
                .map(String::as_str)
                .collect();
            assert_eq!(
                writes,
                [
                    "s/S:stage(all)",
                    "u/U:unstage(all)",
                    "x:discard",
                    "c:commit"
                ]
            );
        }
    }
}

#[test]
fn e2e_vcs_project_log_and_changes_help_filter_and_marks_match_key_behavior() {
    use crate::i18n::{tr, Msg};
    let repo = repo();
    std::fs::write(repo.root.join("file.txt"), "changed\n").unwrap();
    for lang in ["en", "ja"] {
        let cfg = plugin_config("git", "vcs_list_help_supported_keys", lang);
        let (_, mut new) = pair_config(&repo.root, "git", cfg, 160, 20);
        for (screen, query) in [("changes", "FILE.TXT"), ("log", "SEED")] {
            if screen == "log" {
                new.press(KeyCode::Tab, KeyModifiers::NONE);
                drain(&mut new);
            }
            assert_eq!(new.app.project_frame().unwrap().key.screen, screen);
            let prefix = if lang == "en" {
                "Tab/Shift-Tab:screens  j/k:move  /:filter  q:back  ?:help"
            } else {
                "Tab/Shift-Tab:画面  j/k:移動  /:絞り込み  q:戻る  ?:ヘルプ"
            };
            let open_label = if screen == "changes" {
                konoma_plugin::t!(en: "diff", ja: "差分")
            } else {
                konoma_plugin::t!(en: "detail", ja: "詳細")
            };
            let target = if screen == "changes" {
                format!("  Ctrl-t:{}", tr(new.app.lang, Msg::HintNewTab))
            } else {
                String::new()
            };
            let copy = if screen == "log" {
                format!("  y:{}", tr(new.app.lang, Msg::CopyHint))
            } else {
                String::new()
            };
            let expected = format!(
                "{prefix}  ↵/l/→:{}{target}{copy}  Space:{}",
                open_label.get(lang),
                tr(new.app.lang, Msg::WkActionsTitle).trim()
            );
            assert_eq!(crate::ui::status::hint_tokens(&new.app)[0], expected);
            let rows: Vec<_> = crate::ops::help::sections(&new.app)
                .into_iter()
                .flat_map(|s| s.rows)
                .collect();
            assert_eq!(
                rows.iter()
                    .filter(|(keys, label)| keys == "/"
                        && label == tr(new.app.lang, Msg::GitFilterByName))
                    .count(),
                1
            );
            assert!(!rows.iter().any(|(keys, _)| keys == "V"));
            assert!(!new.app.project_marks_enabled());
            new.key('V');
            drain(&mut new);
            assert!(new.app.project_frame().unwrap().marked.is_empty());
            new.key('/');
            new.keys(query);
            drain(&mut new);
            assert!(new.app.project_frame().unwrap().filtering);
            assert_eq!(new.app.project_visible_rows().len(), 1);
            new.keys("no-match");
            drain(&mut new);
            assert!(new.app.project_visible_rows().is_empty());
            new.esc();
            drain(&mut new);
            assert_eq!(new.app.project_visible_rows().len(), 1);
            assert_eq!(new.app.project_frame().unwrap().key.screen, screen);
        }
        new.enter();
        drain(&mut new);
        assert_eq!(new.app.project_frame().unwrap().key.screen, "detail");
        let expected = format!(
            "j/k:{}  h/l:{}  s:{}  g/G:{}  q/Esc:{}  ?:{}  y:{}",
            tr(new.app.lang, Msg::Scroll),
            tr(new.app.lang, Msg::HintHscroll),
            tr(new.app.lang, Msg::FtDiffLayout),
            tr(new.app.lang, Msg::HintEnds),
            tr(new.app.lang, Msg::ProjectBack),
            tr(new.app.lang, Msg::HintHelp),
            tr(new.app.lang, Msg::CopyHint)
        );
        assert_eq!(crate::ui::status::hint_tokens(&new.app), [expected]);
    }
}

#[test]
fn e2e_vcs_worktrees_cells_open_move_filter_hit_and_empty_main_and_linked_both_widths_and_languages(
) {
    let repo = repo();
    for root in [&repo.root, &repo.linked] {
        for lang in ["en", "ja"] {
            for width in [80, 160] {
                let (mut old, mut new) = pair(root, lang, width);
                same(&mut old, &mut new, width);
                assert_eq!(
                    new.app.project_frame().unwrap().cursor,
                    old.app.git_worktree_sel()
                );
                old.key('j');
                new.key('j');
                drain(&mut new);
                same(&mut old, &mut new, width);
                for query in ["日本語", "LINKED", "no-match"] {
                    old.press(KeyCode::Esc, KeyModifiers::NONE);
                    new.press(KeyCode::Esc, KeyModifiers::NONE);
                    // Reopen after Esc closed an unfiltered list.
                    if !old.app.is_git_worktrees() {
                        old.key('w');
                    }
                    if !new.app.project_active() {
                        new.key('p');
                        drain(&mut new);
                        new.key('w');
                        drain(&mut new);
                    }
                    old.key('/');
                    new.key('/');
                    for c in query.chars() {
                        old.key(c);
                        new.key(c);
                    }
                    drain(&mut new);
                    same(&mut old, &mut new, width);
                    if query == "no-match" {
                        let content = new.app.project_content().unwrap();
                        let ProjectContent::List(list) = content.as_ref() else {
                            panic!("list required")
                        };
                        let visible = new.app.project_visible_rows();
                        assert_eq!(visible.len(), 1);
                        assert_eq!(list.rows[visible[0]].kind, "empty");
                        assert!(!list.rows[visible[0]].selectable);
                    }
                }
            }
        }
    }
}

#[test]
fn e2e_vcs_worktrees_diff_header_body_cells_layout_motion_and_back_match_legacy() {
    let repo = repo();
    let body = (0..70)
        .map(|i| format!("line {i} 日本語 {}\n", "long ".repeat(30)))
        .collect::<String>();
    std::fs::write(repo.linked.join("file.txt"), body).unwrap();
    commit(&repo.linked, "feature", 1_577_837_000);
    let uncommitted = (0..70)
        .map(|i| format!("uncommitted {i} {}\n", "long ".repeat(30)))
        .collect::<String>();
    std::fs::write(repo.root.join("file.txt"), uncommitted).unwrap();
    for lang in ["en", "ja"] {
        for width in [80, 160] {
            for path in [&repo.linked, &repo.root] {
                let (mut old, mut new) = pair(&repo.root, lang, width);
                select(&mut old, &mut new, path);
                let cursor = new.app.project_frame().unwrap().cursor;
                old.key('d');
                new.key('d');
                drain(&mut new);
                assert!(old.app.is_git_detail());
                assert_eq!(new.app.project_frame().unwrap().key.screen, "worktree_diff");
                let content = new.app.project_content().unwrap();
                let ProjectContent::Diff(diff) = content.as_ref() else {
                    panic!("diff required")
                };
                assert_eq!(diff.header.len(), 1);
                assert_eq!(
                    diff.header[0].to_string(),
                    old.app.git_detail_title().unwrap()
                );
                let normalize = |lines: &[crate::git::DiffLine]| {
                    lines
                        .iter()
                        .map(|line| (line.kind, line.old_no, line.new_no, line.text.clone()))
                        .collect::<Vec<_>>()
                };
                assert_eq!(
                    normalize(&diff.lines),
                    normalize(old.app.git_detail_lines())
                );
                drop(content);
                let before = old
                    .term
                    .draw(|frame| ui::render(frame, &mut old.app))
                    .unwrap()
                    .buffer
                    .clone();
                let after = new
                    .term
                    .draw(|frame| ui::render(frame, &mut new.app))
                    .unwrap()
                    .buffer
                    .clone();
                let title_width = ratatui::text::Line::raw(old.app.git_detail_title().unwrap())
                    .width()
                    .min(usize::from(width - 4)) as u16;
                for x in 0..title_width {
                    let a = &before[(x + 2, 1)];
                    let b = &after[(x, 2)];
                    assert_eq!(
                        (a.symbol(), a.fg, a.bg, a.modifier),
                        (b.symbol(), b.fg, b.bg, b.modifier)
                    );
                }
                same_diff(&mut old, &mut new, width);
                for key in ['j', 'l', 's'] {
                    old.key(key);
                    new.key(key);
                    drain(&mut new);
                    same_diff(&mut old, &mut new, width);
                }
                old.key('q');
                new.key('q');
                drain(&mut new);
                assert_eq!(new.app.project_frame().unwrap().key.screen, "worktrees");
                assert_eq!(new.app.project_frame().unwrap().cursor, cursor);
                same(&mut old, &mut new, width);
            }
        }
    }
}

#[test]
fn e2e_vcs_worktrees_create_works_on_empty_filter_and_blank_does_not_reopen() {
    let repo = repo();
    let (mut old, mut new) = pair(&repo.root, "en", 160);
    old.key('/');
    new.key('/');
    old.keys("no-match");
    new.keys("no-match");
    old.enter();
    new.enter();
    drain(&mut new);
    submit(&mut old, &mut new, "   ");
    assert_eq!(new.app.flash, old.app.flash);
    assert!(new.app.project_dialog.is_none());
    new.key('c');
    new.keys("from-empty");
    new.enter();
    drain(&mut new);
    assert_eq!(new.app.tab.root, repo._dir.join("from-empty"));
    assert!(new.app.tab.root.join(".git").is_file());
}

#[test]
fn e2e_vcs_worktrees_diff_empty_and_refused_rows_flash_without_push() {
    let repo = repo();
    git(&repo._dir, &["clone", "-q", "--bare", "main", "bare"]);
    let bare = repo._dir.join("bare");
    let linked = repo._dir.join("bare-linked");
    git(
        &bare,
        &[
            "worktree",
            "add",
            "-q",
            "--detach",
            linked.to_str().unwrap(),
        ],
    );
    for lang in ["en", "ja"] {
        for (root, path) in [
            (&repo.root, repo.root.clone()),
            (&repo.root, repo._dir.join("prunable")),
            (&linked, bare.clone()),
        ] {
            let (mut old, mut new) = pair(root, lang, 160);
            select(&mut old, &mut new, &path);
            let cursor = new.app.project_frame().unwrap().cursor;
            old.key('d');
            new.key('d');
            drain(&mut new);
            assert!(!old.app.is_git_detail());
            assert_eq!(new.app.project_frame().unwrap().key.screen, "worktrees");
            assert_eq!(new.app.project_frame().unwrap().cursor, cursor);
            assert_eq!(new.app.flash, old.app.flash);
            assert_eq!(&new.app.tab.root, root);
        }
    }
}

#[test]
fn e2e_vcs_worktrees_read_only_blocks_create_before_prompt_but_all_navigation_and_diff_work() {
    let repo = repo();
    std::fs::write(repo.linked.join("file.txt"), "changed\n").unwrap();
    for lang in ["en", "ja"] {
        for operation in ["enter", "tab", "diff"] {
            let (mut old, mut new) = pair_settings(&repo.root, lang, 160, "../", true, &[]);
            old.app
                .cfg
                .plugins
                .entry("git".into())
                .or_default()
                .insert("read_only".into(), toml::Value::Boolean(true));
            select(&mut old, &mut new, &repo.linked);
            let count = crate::git::worktrees(&repo.root).len();
            old.key('c');
            new.key('c');
            drain(&mut new);
            assert_eq!(new.app.flash, old.app.flash);
            assert_eq!(
                new.app.flash.as_deref(),
                Some(crate::i18n::tr(
                    new.app.lang,
                    crate::i18n::Msg::PluginReadOnly
                ))
            );
            assert!(!old.app.is_dialog() && new.app.project_dialog.is_none());
            assert_eq!(crate::git::worktrees(&repo.root).len(), count);
            match operation {
                "enter" => {
                    new.enter();
                    drain(&mut new);
                    assert_eq!(new.app.tab.root, repo.linked);
                }
                "tab" => {
                    new.ctrl('t');
                    drain(&mut new);
                    assert_eq!(new.app.tab_count(), 2);
                    assert_eq!(new.app.tab.root, repo.linked);
                }
                _ => {
                    new.key('d');
                    drain(&mut new);
                    assert_eq!(new.app.project_frame().unwrap().key.screen, "worktree_diff");
                }
            }
        }
    }
}

#[test]
fn e2e_vcs_worktrees_help_matches_legacy_action_texts_in_both_languages_with_only_target_exception()
{
    use crate::keymap::{Action, Backend, Context, Screen};
    let repo = repo();
    for lang in ["en", "ja"] {
        for path in [&repo.root, &repo.linked] {
            let (mut old, mut new) = pair(&repo.root, lang, 160);
            select(&mut old, &mut new, path);
            let actions = [
                Action::WorktreeGoto,
                Action::WorktreeCreate,
                Action::WorktreeGotoNewTab,
                Action::WorktreeShowChanges,
                Action::WorktreeFilterStart,
            ];
            let mut expected: Vec<_> = crate::ops::help::rows_for(
                &old.app,
                Context::Screen(Backend::Git, Screen::Worktrees),
            )
            .into_iter()
            .filter(|row| row.actions.iter().any(|a| actions.contains(a)))
            .map(|row| {
                (
                    if row.keys_text.contains("Enter") {
                        "Enter".into()
                    } else {
                        row.keys_text
                    },
                    row.desc,
                )
            })
            .collect();
            if path == &repo.root {
                expected.retain(|(key, _)| key != "Ctrl-t");
            }
            expected.sort();
            let sections = crate::ops::help::sections(&new.app);
            let mut actual: Vec<_> = sections
                .iter()
                .flat_map(|s| &s.rows)
                .filter(|(keys, _)| ["Enter", "c", "Ctrl-t", "d", "/"].contains(&keys.as_str()))
                .cloned()
                .collect();
            actual.sort();
            assert_eq!(actual, expected);
            assert!(!sections
                .iter()
                .flat_map(|s| &s.rows)
                .any(|(keys, _)| keys == "V"));
        }
    }
}
