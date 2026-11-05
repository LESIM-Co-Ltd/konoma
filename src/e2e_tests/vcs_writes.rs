//! Real repository writes through handle_key, compared with the legacy changes hub.

use super::*;
use crate::app::project_screens::{ProjectContent, ProjectDialog, ProjectPrompt};
use crate::i18n::{tr, Msg};
use crate::keymap::{Action, Backend, Context, Screen};
use crate::plugins::test_projects::{ObservedVcs, VcsCalls};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

fn git_output(root: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .current_dir(root)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "Fixture")
        .env("GIT_AUTHOR_EMAIL", "fixture@example.invalid")
        .env("GIT_COMMITTER_NAME", "Fixture")
        .env("GIT_COMMITTER_EMAIL", "fixture@example.invalid")
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

fn seed(root: &Path, changed: bool) {
    std::fs::create_dir_all(root).unwrap();
    git_output(
        root,
        &["-c", "init.defaultBranch=main", "init", "-q", "--template="],
    );
    for (key, value) in [
        ("user.name", "Fixture"),
        ("user.email", "fixture@example.invalid"),
        ("commit.gpgsign", "false"),
        ("core.hooksPath", ".git/hooks"),
        ("core.quotePath", "false"),
    ] {
        git_output(root, &["config", "--local", key, value]);
    }
    std::fs::create_dir_all(root.join("sub")).unwrap();
    for file in ["a-modified", "c-deleted", "d-old", "sub/日本語.txt"] {
        // Distinct contents keep rename detection from pairing an unrelated deleted file.
        std::fs::write(root.join(file), format!("base {file}\n")).unwrap();
    }
    git_output(root, &["add", "."]);
    git_output(root, &["commit", "-qm", "seed"]);
    if changed {
        std::fs::write(root.join("a-modified"), "changed\n").unwrap();
        std::fs::write(root.join("b-new"), "new\n").unwrap();
        std::fs::remove_file(root.join("c-deleted")).unwrap();
        git_output(root, &["mv", "d-old", "d-renamed"]);
        std::fs::write(root.join("sub/日本語.txt"), "変更\n").unwrap();
    }
}

struct WritePair {
    old: Sim,
    new: Sim,
    old_root: PathBuf,
    new_root: PathBuf,
    calls: Arc<Mutex<VcsCalls>>,
    _dir: crate::test_support::TmpDir,
}

impl WritePair {
    fn git(name: &str, changed: bool, lang: &str, style: &str, read_only: bool) -> Self {
        let dir = sandbox(name).into_canonical().unwrap();
        let old_root = dir.join("old/repo");
        let new_root = dir.join("new/repo");
        seed(&old_root, changed);
        seed(&new_root, changed);
        Self::open(
            dir, old_root, new_root, "git", name, lang, style, read_only, true,
        )
    }

    fn jj(name: &str, lang: &str, confirm: bool, read_only: bool, colocated: bool) -> Option<Self> {
        if !crate::vcs::jj::available() {
            return None;
        }
        let dir = sandbox(name).into_canonical().unwrap();
        let old_root = dir.join("old/repo");
        let new_root = dir.join("new/repo");
        for root in [&old_root, &new_root] {
            if colocated {
                seed(root, false);
            } else {
                std::fs::create_dir_all(root).unwrap();
            }
            assert!(crate::test_support::run_jj(
                root,
                &[
                    "git",
                    "init",
                    if colocated {
                        "--colocate"
                    } else {
                        "--no-colocate"
                    },
                    "."
                ]
            ));
            std::fs::write(root.join("a-modified"), "base\n").unwrap();
            assert!(crate::test_support::run_jj(
                root,
                &["commit", "-m", "jj seed"]
            ));
            std::fs::write(root.join("a-modified"), "changed\n").unwrap();
        }
        Some(Self::open(
            dir, old_root, new_root, "jj", name, lang, "relative", read_only, confirm,
        ))
    }

    #[allow(clippy::too_many_arguments)]
    fn open(
        dir: crate::test_support::TmpDir,
        old_root: PathBuf,
        new_root: PathBuf,
        id: &str,
        name: &str,
        lang: &str,
        style: &str,
        read_only: bool,
        confirm: bool,
    ) -> Self {
        let calls = Arc::new(Mutex::new(VcsCalls::default()));
        let plugin: Arc<dyn Plugin> = if id == "git" {
            Arc::new(konoma_vcs::Git)
        } else {
            Arc::new(konoma_vcs::Jj)
        };
        // Each registry cache entry must own this fixture's observer, including loop iterations.
        let registration = format!("{name}-{}", dir.file_name().unwrap().to_string_lossy());
        let token = test_plugins::register(
            &registration,
            Arc::new(ObservedVcs {
                plugin,
                calls: calls.clone(),
            }),
        );
        let mut cfg = test_plugins::marker_config(&token);
        cfg.ui.lang = lang.into();
        cfg.ui.path_style = style.into();
        cfg.ui.statusbar = "split".into();
        cfg.ui.tabbar = "hidden".into();
        cfg.ui.confirm_jj_sync = confirm;
        let settings = cfg.plugins.entry(id.into()).or_default();
        settings.insert("read_only".into(), toml::Value::Boolean(read_only));
        settings.insert("confirm_sync".into(), toml::Value::Boolean(confirm));
        settings.insert("path_style".into(), toml::Value::String(style.into()));
        let mut old_cfg = cfg.clone();
        old_cfg.plugins.remove(&token);
        let mut old = Sim::with_config_sized(&old_root, old_cfg, 200, 24);
        old.app.open_git_view();
        assert!(old.app.is_git_view(), "id={id} flash={:?}", old.app.flash);
        old.draw();
        let mut new = Sim::with_config_sized(&new_root, cfg, 200, 24);
        new.app.tab.projects.wait_for_result();
        new.app.poll_projects();
        new.key('p');
        drain(&mut new);
        assert!(new.app.project_active(), "{:?}", new.app.flash);
        Self {
            old,
            new,
            old_root,
            new_root,
            calls,
            _dir: dir,
        }
    }

    fn key(&mut self, key: char) {
        self.old.key(key);
        self.new.key(key);
        drain(&mut self.new);
    }

    fn press_before_draw(&mut self, code: KeyCode) {
        self.old.press(code, KeyModifiers::NONE);
        handle_key(&mut self.new.app, KeyEvent::new(code, KeyModifiers::NONE)).unwrap();
        for _ in 0..32 {
            self.new.app.poll_projects();
            if !self.new.app.project_jobs_pending() {
                return;
            }
            self.new.app.wait_project_job();
        }
        panic!("VCS action did not finish");
    }

    fn select(&mut self, relative: &str) {
        self.key('g');
        let index = self
            .old
            .app
            .git_view_entries()
            .iter()
            .position(|e| e.path == self.old_root.join(relative))
            .unwrap_or_else(|| {
                panic!(
                    "missing {relative}; rows={:?}",
                    self.old.app.git_view_entries()
                )
            });
        for _ in 0..index {
            self.key('j');
        }
        assert_eq!(
            self.old.app.git_view_selected(),
            Some(self.old_root.join(relative))
        );
        let content = self.new.app.project_content().unwrap();
        let ProjectContent::List(list) = content.as_ref() else {
            panic!("not a list")
        };
        assert_eq!(
            list.rows[index].value,
            self.new_root.join(relative).to_string_lossy()
        );
    }

    fn assert_same(&mut self) {
        assert_eq!(
            git_output(&self.new_root, &["status", "--porcelain=v1"]),
            git_output(&self.old_root, &["status", "--porcelain=v1"])
        );
        assert_eq!(
            git_output(&self.new_root, &["log", "--format=%s"]),
            git_output(&self.old_root, &["log", "--format=%s"])
        );
        let normalize = |value: Option<&str>, root: &Path| {
            value.map(|s| s.replace(root.to_str().unwrap(), "<repo>"))
        };
        assert_eq!(
            normalize(self.new.app.flash.as_deref(), &self.new_root),
            normalize(self.old.app.flash.as_deref(), &self.old_root)
        );
        self.assert_rows();
    }

    fn assert_rows(&self) {
        let expected: Vec<_> = self
            .old
            .app
            .git_view_entries()
            .iter()
            .map(|e| {
                format!(
                    "{}{} {}",
                    if e.staged { "● " } else { "  " },
                    e.status.marker(),
                    self.old.app.format_path(&e.path)
                )
                .replace(self.old_root.to_str().unwrap(), "<repo>")
            })
            .collect();
        let expected = if expected.is_empty() {
            vec![tr(self.old.app.lang, Msg::GitNoChangesItem).into()]
        } else {
            expected
        };
        let content = self.new.app.project_content().unwrap();
        let ProjectContent::List(list) = content.as_ref() else {
            panic!("not a list")
        };
        let actual: Vec<_> = list
            .rows
            .iter()
            .map(|row| {
                row.cells[0]
                    .plain()
                    .replace(self.new_root.to_str().unwrap(), "<repo>")
            })
            .collect();
        assert_eq!(actual, expected);
    }

    fn assert_confirmation(&mut self) {
        let (true, message, _, _) = self.old.app.dialog_view().unwrap() else {
            panic!("not a confirmation")
        };
        let Some(ProjectDialog::Act {
            invocation,
            prompts,
            stage,
            ..
        }) = &self.new.app.project_dialog
        else {
            panic!("missing confirmation")
        };
        let ProjectPrompt::Confirmation(prompt) = &prompts[*stage] else {
            panic!("not a confirmation")
        };
        assert_eq!(
            invocation.confirmation_text(prompt, self.new.app.plugin_lang()),
            message
        );
        // TestBackend leaves a blank continuation cell after each wide character.
        let needle: String = message
            .chars()
            .flat_map(|c| {
                std::iter::once(c).chain(std::iter::repeat_n(
                    ' ',
                    unicode_width::UnicodeWidthChar::width(c)
                        .unwrap_or(1)
                        .saturating_sub(1),
                ))
            })
            .collect();
        self.old.see(&needle);
        self.new.see(&needle);
    }

    fn assert_input(&self, text: &str) {
        let (false, title, buffer, _) = self.old.app.dialog_view().unwrap() else {
            panic!("not an input")
        };
        assert_eq!(buffer, text);
        let Some(ProjectDialog::Act {
            prompts,
            stage,
            buffer,
            ..
        }) = &self.new.app.project_dialog
        else {
            panic!("missing input")
        };
        let ProjectPrompt::Text(prompt) = &prompts[*stage] else {
            panic!("not an input")
        };
        assert_eq!(prompt.label.get(self.new.app.plugin_lang()), title);
        assert_eq!(buffer, text);
    }
}

#[test]
fn e2e_vcs_writes_stage_unstage_modified_new_deleted_renamed_cjk_and_path_styles() {
    for lang in ["en", "ja"] {
        for style in ["relative", "home", "full"] {
            for file in [
                "a-modified",
                "b-new",
                "c-deleted",
                "d-renamed",
                "sub/日本語.txt",
            ] {
                let mut p = WritePair::git("vcs_write_rows", true, lang, style, false);
                p.select(file);
                for key in ['s', 'u'] {
                    p.key(key);
                    assert!(p.old.app.flash.is_some());
                    p.assert_same();
                    if key == 's' {
                        p.select(file);
                    }
                }
            }
        }
    }
}

#[test]
fn e2e_vcs_writes_all_and_empty_guards_match_without_running_git() {
    for lang in ["en", "ja"] {
        for changed in [false, true] {
            let mut p = WritePair::git("vcs_write_all", changed, lang, "relative", false);
            if !changed {
                // An attempted index write must fail, even if it would leave status unchanged.
                for root in [&p.old_root, &p.new_root] {
                    std::fs::write(root.join(".git/index.lock"), "fixture lock").unwrap();
                }
            }
            for key in ['S', 'U', 'U'] {
                p.key(key);
                p.assert_same();
                let msg = if key == 'S' && !changed {
                    Msg::NoChanges
                } else if key == 'U'
                    && (!changed
                        || p.old.app.flash.as_deref()
                            == Some(tr(p.old.app.lang, Msg::NothingStaged)))
                {
                    Msg::NothingStaged
                } else {
                    continue;
                };
                assert_eq!(p.new.app.flash.as_deref(), Some(tr(p.new.app.lang, msg)));
            }
        }
    }
}

#[test]
fn e2e_vcs_writes_empty_file_keys_never_invoke_git_or_flash() {
    let mut p = WritePair::git("vcs_write_empty_keys", false, "en", "relative", false);
    let before = git_output(&p.new_root, &["status", "--porcelain=v1"]);
    let index = std::fs::read(p.new_root.join(".git/index")).unwrap();
    let calls = p.calls.lock().unwrap().acts.len();
    for key in ['s', 'u', 'x'] {
        p.key(key);
        assert!(p.old.app.flash.is_none());
        assert!(p.new.app.flash.is_none());
        assert!(!p.old.app.is_dialog());
        assert!(p.new.app.project_dialog.is_none());
        assert_eq!(
            p.calls.lock().unwrap().acts.len(),
            calls,
            "no action invocation means no git call"
        );
        assert_eq!(std::fs::read(p.new_root.join(".git/index")).unwrap(), index);
        assert_eq!(
            git_output(&p.new_root, &["status", "--porcelain=v1"]),
            before
        );
        p.assert_same();
    }
}

#[test]
fn e2e_vcs_writes_discard_confirmation_and_failures() {
    for lang in ["en", "ja"] {
        for file in ["a-modified", "sub/日本語.txt"] {
            let mut p = WritePair::git("vcs_write_discard", true, lang, "relative", false);
            p.select(file);
            p.key('x');
            p.assert_confirmation();
            p.key('y');
            p.assert_same();
            assert_eq!(
                std::fs::read_to_string(p.new_root.join(file)).unwrap(),
                format!("base {file}\n")
            );
        }
        let mut p = WritePair::git("vcs_write_failures", true, lang, "relative", false);
        p.select("b-new");
        p.key('x');
        p.assert_confirmation();
        p.key('y');
        p.assert_same();
        assert!(p
            .new
            .app
            .flash
            .as_ref()
            .unwrap()
            .starts_with(&format!("{}: ", tr(p.new.app.lang, Msg::Failed))));
        for root in [&p.old_root, &p.new_root] {
            std::fs::write(root.join(".git/index.lock"), "fixture lock").unwrap();
        }
        for key in ['s', 'u', 'S', 'U'] {
            p.key(key);
            p.assert_same();
            assert!(p
                .new
                .app
                .flash
                .as_ref()
                .unwrap()
                .starts_with(&format!("{}: ", tr(p.new.app.lang, Msg::Failed))));
        }
    }
}

fn discard_cancel(key: KeyCode) {
    for lang in ["en", "ja"] {
        let mut p = WritePair::git("vcs_write_discard_cancel", true, lang, "relative", false);
        p.select("a-modified");
        let before = git_output(&p.new_root, &["status", "--porcelain=v1"]);
        p.key('x');
        p.assert_confirmation();
        p.old.press(key, KeyModifiers::NONE);
        p.new.press(key, KeyModifiers::NONE);
        drain(&mut p.new);
        assert!(p.calls.lock().unwrap().acts.is_empty());
        assert_eq!(
            git_output(&p.new_root, &["status", "--porcelain=v1"]),
            before
        );
        assert_eq!(
            p.old.app.flash.as_deref(),
            Some(tr(p.old.app.lang, Msg::Canceled))
        );
        // Keep the legacy cancellation contract strict, including its flash and no dispatch.
        p.assert_same();
        assert!(!p.old.app.is_dialog());
        assert!(p.new.app.project_dialog.is_none());
    }
}

#[test]
fn e2e_vcs_writes_discard_cancel_n_matches_legacy_flash_and_closes() {
    discard_cancel(KeyCode::Char('n'));
    discard_cancel(KeyCode::Char('N'));
}

#[test]
fn e2e_vcs_writes_discard_cancel_escape_matches_legacy_flash_and_closes() {
    discard_cancel(KeyCode::Esc);
}

#[test]
fn e2e_vcs_writes_discard_enter_does_nothing_and_uppercase_y_confirms() {
    for lang in ["en", "ja"] {
        let mut p = WritePair::git(
            "vcs_write_discard_confirm_keys",
            true,
            lang,
            "relative",
            false,
        );
        p.select("a-modified");
        let before = git_output(&p.new_root, &["status", "--porcelain=v1"]);
        p.key('x');
        p.assert_confirmation();
        p.press_before_draw(KeyCode::Enter);
        drain(&mut p.new);
        p.assert_confirmation();
        p.assert_same();
        assert!(p.calls.lock().unwrap().acts.is_empty());
        assert_eq!(
            git_output(&p.new_root, &["status", "--porcelain=v1"]),
            before
        );
        p.key('Y');
        assert!(!p.old.app.is_dialog());
        assert!(p.new.app.project_dialog.is_none());
        assert_eq!(p.calls.lock().unwrap().acts, ["discard"]);
        assert_eq!(
            std::fs::read_to_string(p.new_root.join("a-modified")).unwrap(),
            "base a-modified\n"
        );
        p.assert_same();
    }
}

#[test]
fn e2e_vcs_writes_commit_success_empty_and_failed_hook_preserves_prompt() {
    for lang in ["en", "ja"] {
        for fail in [false, true] {
            let mut p = WritePair::git("vcs_write_commit", true, lang, "relative", false);
            p.key('S');
            p.key('c');
            p.assert_input("");
            p.key(' ');
            p.old.enter();
            p.new.enter();
            drain(&mut p.new);
            p.assert_same();
            assert_eq!(
                p.new.app.flash.as_deref(),
                Some(tr(p.new.app.lang, Msg::MessageEmpty))
            );
            assert!(!p.old.app.is_dialog());
            assert!(p.new.app.project_dialog.is_none());
            if fail {
                for root in [&p.old_root, &p.new_root] {
                    std::fs::create_dir_all(root.join(".git/hooks")).unwrap();
                    crate::test_support::write_executable(
                        &root.join(".git/hooks/pre-commit"),
                        "#!/bin/sh\necho fixture-hook-refused >&2\nexit 1\n",
                    );
                }
            }
            p.key('c');
            for key in "fixture commit 日本語".chars() {
                p.key(key);
            }
            p.old.enter();
            p.new.enter();
            drain(&mut p.new);
            p.assert_same();
            if fail {
                assert_eq!(p.new.app.flash.as_deref(), Some("fixture-hook-refused"));
                p.assert_input("fixture commit 日本語");
            } else {
                assert_eq!(
                    p.new.app.flash.as_deref(),
                    Some(tr(p.new.app.lang, Msg::Committed))
                );
                assert!(git_output(&p.new_root, &["log", "-1", "--format=%s"])
                    .contains("fixture commit 日本語"));
            }
        }
    }
}

#[test]
fn e2e_vcs_writes_read_only_refuses_every_git_write_before_prompt_or_dispatch() {
    for lang in ["en", "ja"] {
        let mut p = WritePair::git("vcs_write_read_only", true, lang, "relative", true);
        for key in ['s', 'u', 'S', 'U', 'x', 'c'] {
            p.key(key);
            p.assert_same();
            assert_eq!(
                p.new.app.flash.as_deref(),
                Some(tr(p.new.app.lang, Msg::PluginReadOnly))
            );
            assert!(!p.old.app.is_dialog());
            assert!(p.new.app.project_dialog.is_none());
            assert!(p.calls.lock().unwrap().acts.is_empty());
        }
    }
}

#[test]
fn e2e_vcs_writes_refresh_current_screen_tree_status_and_commit_chip() {
    let mut p = WritePair::git("vcs_write_refresh", true, "en", "relative", false);
    p.select("a-modified");
    for key in ['s', 'u', 'S', 'U', 'x'] {
        let views = p.calls.lock().unwrap().views;
        p.new.app.clear_project_tree_dirty();
        p.press_before_draw(KeyCode::Char(key));
        if key == 'x' {
            p.press_before_draw(KeyCode::Char('y'));
        }
        assert!(
            p.new.app.project_tree_dirty(),
            "{key}: tree status not invalidated"
        );
        assert_eq!(
            p.calls.lock().unwrap().views,
            views + 1,
            "{key}: changes list not refreshed"
        );
        drain(&mut p.new);
        p.assert_same();
    }
    p.key('S');
    let (views, chips) = {
        let calls = p.calls.lock().unwrap();
        (calls.views, calls.chips)
    };
    p.new.app.clear_project_tree_dirty();
    p.key('c');
    p.key('m');
    p.press_before_draw(KeyCode::Enter);
    assert!(p.new.app.project_tree_dirty());
    assert_eq!(p.calls.lock().unwrap().views, views + 1);
    assert_eq!(p.calls.lock().unwrap().chips, chips + 1);
    drain(&mut p.new);
    p.assert_same();
}

fn write_hint(token: &str) -> bool {
    token
        .split_once(':')
        .is_some_and(|(keys, _)| matches!(keys, "s/S" | "u/U" | "S" | "U" | "s" | "u" | "x" | "c"))
}

fn assert_hints(p: &WritePair, jj: bool, empty: bool) {
    let ctx = Context::Screen(if jj { Backend::Jj } else { Backend::Git }, Screen::Changes);
    let old_tokens = crate::ops::footer::tokens(&p.old.app, ctx);
    let mut expected: Vec<_> = old_tokens.into_iter().filter(|s| write_hint(s)).collect();
    let actual: Vec<_> = crate::ui::status::hint_tokens(&p.new.app)
        .into_iter()
        .filter(|s| write_hint(s))
        .collect();
    let actions = [
        Action::GitStage,
        Action::GitUnstage,
        Action::GitDiscard,
        Action::GitCommit,
        Action::JjSync,
    ];
    let mut help: Vec<_> = crate::ops::help::rows_for(&p.old.app, ctx)
        .into_iter()
        .filter(|row| row.actions.iter().any(|a| actions.contains(a)))
        .map(|row| (row.keys_text, row.desc))
        .collect();
    if empty && !jj {
        // User decision (2026-11-05): only the inert s/u/x hints on an empty list differ.
        // Assert the exact legacy shape before applying this narrow exception.
        assert_eq!(expected.len(), 4);
        assert!(expected[0].starts_with("s/S:"));
        assert!(expected[1].starts_with("u/U:"));
        assert!(expected[2].starts_with("x:"));
        expected[0] = expected[0].replacen("s/S:", "S:", 1);
        expected[1] = expected[1].replacen("u/U:", "U:", 1);
        expected.remove(2);
        assert_eq!(help.len(), 4);
        assert_eq!(help[0].0, "s / S");
        assert_eq!(help[1].0, "u / U");
        assert_eq!(help[2].0, "x");
        help[0].0 = "S".into();
        help[1].0 = "U".into();
        help.remove(2);
    }
    assert_eq!(actual, expected);
    let actual_help: Vec<_> = crate::ops::help::sections_for(&p.new.app, Context::Canvas)
        .into_iter()
        .filter(|section| section.title == if jj { "jj" } else { "Git" })
        .flat_map(|section| section.rows)
        .filter(|(keys, _)| keys != "Enter")
        .collect();
    assert_eq!(actual_help, help);
}

#[test]
fn e2e_vcs_writes_footer_help_exact_except_empty_inert_file_keys() {
    for lang in ["en", "ja"] {
        for changed in [false, true] {
            let mut p = WritePair::git("vcs_write_hints", changed, lang, "relative", false);
            assert_hints(&p, false, !changed);
            if changed {
                p.select("d-renamed");
                assert_hints(&p, false, false);
                p.select("a-modified");
                assert_hints(&p, false, false);
            }
        }
    }
}

struct JjPreference;

impl JjPreference {
    fn pin() -> Self {
        crate::vcs::set_preference_for_test(Some(crate::vcs::Preference::Jj));
        Self
    }
}

impl Drop for JjPreference {
    fn drop(&mut self) {
        crate::vcs::set_preference_for_test(None);
    }
}

fn jj_output(root: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("jj")
        .current_dir(root)
        .env("HOME", root)
        .env("JJ_CONFIG", "/dev/null")
        .env("JJ_USER", "Fixture")
        .env("JJ_EMAIL", "fixture@example.invalid")
        .args(["--ignore-working-copy", "--color=never"])
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "jj {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout)
        .unwrap()
        .replace(root.to_str().unwrap(), "<repo>")
}

fn jj_state(root: &Path) -> Vec<String> {
    vec![
        jj_output(root, &["file", "show", "-r", "@", "a-modified"]),
        jj_output(
            root,
            &[
                "log",
                "--no-graph",
                "-r",
                "all()",
                "-T",
                "description ++ \"\\n\"",
            ],
        ),
        jj_output(
            root,
            &["op", "log", "--no-graph", "-T", "description ++ \"\\n\""],
        )
        .lines()
        .map(|line| {
            // Separate fixtures have different commit IDs; retain the operation and its order.
            match line.strip_prefix("commit ") {
                Some(id) if id.len() == 40 && id.bytes().all(|b| b.is_ascii_hexdigit()) => {
                    "commit <revision>"
                }
                _ => line,
            }
        })
        .collect::<Vec<_>>()
        .join("\n"),
    ]
}

fn assert_jj_same(p: &WritePair, colocated: bool) {
    assert_eq!(jj_state(&p.new_root), jj_state(&p.old_root));
    assert_eq!(p.new.app.flash, p.old.app.flash);
    p.assert_rows();
    if colocated {
        assert_eq!(
            git_output(&p.new_root, &["status", "--porcelain=v1"]),
            git_output(&p.old_root, &["status", "--porcelain=v1"])
        );
        assert_eq!(
            git_output(&p.new_root, &["log", "--format=%s"]),
            git_output(&p.old_root, &["log", "--format=%s"])
        );
    }
}

#[test]
fn e2e_vcs_writes_jj_sync_confirmation_cancel_enter_and_uppercase_y() {
    let _preference = JjPreference::pin();
    for lang in ["en", "ja"] {
        for colocated in [false, true] {
            let Some(mut p) = WritePair::jj("vcs_write_jj_confirm", lang, true, false, colocated)
            else {
                return;
            };
            assert_hints(&p, true, false);
            let before = jj_state(&p.new_root);
            assert_eq!(before[0], "base\n", "reading changes must not snapshot");
            for cancel in [KeyCode::Char('n'), KeyCode::Char('N'), KeyCode::Esc] {
                p.key('s');
                p.assert_confirmation();
                p.old.press(cancel, KeyModifiers::NONE);
                p.new.press(cancel, KeyModifiers::NONE);
                drain(&mut p.new);
                assert!(p.calls.lock().unwrap().acts.is_empty());
                assert_eq!(jj_state(&p.new_root), before);
                assert!(!p.old.app.is_dialog());
                assert!(p.new.app.project_dialog.is_none());
                assert_eq!(
                    p.new.app.flash.as_deref(),
                    Some(tr(p.new.app.lang, Msg::Canceled))
                );
                assert_jj_same(&p, colocated);
            }
            p.key('s');
            p.assert_confirmation();
            p.press_before_draw(KeyCode::Enter);
            drain(&mut p.new);
            p.assert_confirmation();
            assert!(p.calls.lock().unwrap().acts.is_empty());
            assert_eq!(jj_state(&p.new_root), before);
            assert_jj_same(&p, colocated);
            let (views, chips) = {
                let calls = p.calls.lock().unwrap();
                (calls.views, calls.chips)
            };
            p.new.app.clear_project_tree_dirty();
            p.press_before_draw(KeyCode::Char('Y'));
            assert!(p.new.app.project_tree_dirty());
            assert_eq!(p.calls.lock().unwrap().views, views + 1);
            assert_eq!(p.calls.lock().unwrap().chips, chips + 1);
            drain(&mut p.new);
            assert!(!p.old.app.is_dialog());
            assert!(p.new.app.project_dialog.is_none());
            assert_eq!(p.calls.lock().unwrap().acts, ["sync"]);
            assert_eq!(
                p.new.app.flash.as_deref(),
                Some(tr(p.new.app.lang, Msg::JjSyncDone))
            );
            assert_eq!(jj_state(&p.new_root)[0], "changed\n");
            assert_jj_same(&p, colocated);
        }
    }
}

#[test]
fn e2e_vcs_writes_jj_sync_setting_false_snapshots_without_prompt() {
    let _preference = JjPreference::pin();
    for lang in ["en", "ja"] {
        for colocated in [false, true] {
            let Some(mut p) =
                WritePair::jj("vcs_write_jj_no_confirm", lang, false, false, colocated)
            else {
                return;
            };
            assert_hints(&p, true, false);
            assert_eq!(jj_state(&p.new_root)[0], "base\n");
            p.key('s');
            assert!(!p.old.app.is_dialog());
            assert!(p.new.app.project_dialog.is_none());
            assert_eq!(p.calls.lock().unwrap().acts, ["sync"]);
            assert_eq!(
                p.new.app.flash.as_deref(),
                Some(tr(p.new.app.lang, Msg::JjSyncDone))
            );
            assert_eq!(jj_state(&p.new_root)[0], "changed\n");
            assert_jj_same(&p, colocated);
        }
    }
}

#[test]
fn e2e_vcs_writes_jj_sync_read_only_refuses_before_prompt_or_snapshot() {
    let _preference = JjPreference::pin();
    for lang in ["en", "ja"] {
        for confirm in [false, true] {
            let Some(mut p) = WritePair::jj("vcs_write_jj_read_only", lang, confirm, true, false)
            else {
                return;
            };
            assert_hints(&p, true, false);
            let before = jj_state(&p.new_root);
            p.key('s');
            assert!(!p.old.app.is_dialog());
            assert!(p.new.app.project_dialog.is_none());
            assert!(p.calls.lock().unwrap().acts.is_empty());
            assert_eq!(
                p.new.app.flash.as_deref(),
                Some(tr(p.new.app.lang, Msg::PluginReadOnly))
            );
            assert_eq!(jj_state(&p.new_root), before);
            assert_jj_same(&p, false);
        }
    }
}
