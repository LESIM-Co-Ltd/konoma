//! Literal expectations for the VCS writes on the project canvas (git and jj), driven through the
//! host's `p` entry only. Nothing here opens the old hub or compares against it: rows, flashes,
//! confirmation texts, repository bytes and call counts are written out as values.
use super::vcs_changes::{
    drain, seed_jj_write_repository, seed_write_repository, write_git_output, write_jj_state,
};
use super::*;
use crate::app::project_screens::{ProjectContent, ProjectDialog, ProjectPrompt};
use crate::keymap::Context;
use crate::plugins::test_projects::{ObservedVcs, VcsCalls};
use konoma_plugin::Plugin;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

struct W {
    sim: Sim,
    root: PathBuf,
    calls: Arc<Mutex<VcsCalls>>,
    _dir: crate::test_support::TmpDir,
}

#[allow(clippy::too_many_arguments)]
fn open_repo(
    dir: crate::test_support::TmpDir,
    root: PathBuf,
    id: &str,
    name: &str,
    lang: &str,
    style: &str,
    read_only: bool,
    confirm: bool,
) -> W {
    let calls = Arc::new(Mutex::new(VcsCalls::default()));
    let plugin: Arc<dyn Plugin> = if id == "git" {
        Arc::new(konoma_vcs::Git)
    } else {
        Arc::new(konoma_vcs::Jj)
    };
    // Each registry entry must own this fixture's observer, including loop iterations.
    let registration = format!("{name}-{}", dir.file_name().unwrap().to_string_lossy());
    let token = crate::plugins::test_plugins::register_replacement(
        &registration,
        Arc::new(ObservedVcs {
            plugin,
            calls: calls.clone(),
        }),
    );
    let mut cfg = crate::plugins::test_plugins::marker_config(&token);
    let other = if id == "git" { "jj" } else { "git" };
    cfg.plugins
        .entry(other.into())
        .or_default()
        .insert("enabled".into(), toml::Value::Boolean(false));
    cfg.ui.lang = lang.into();
    cfg.ui.path_style = style.into();
    cfg.ui.statusbar = "split".into();
    cfg.ui.tabbar = "hidden".into();
    cfg.ui.confirm_jj_sync = confirm;
    let settings = cfg.plugins.entry(id.into()).or_default();
    settings.insert("read_only".into(), toml::Value::Boolean(read_only));
    settings.insert("confirm_sync".into(), toml::Value::Boolean(confirm));
    settings.insert("path_style".into(), toml::Value::String(style.into()));
    let mut sim = Sim::with_config_sized(&root, cfg, 200, 24);
    sim.app.tab.projects.wait_for_result();
    sim.app.poll_projects();
    sim.key('p');
    drain(&mut sim);
    assert!(sim.app.project_active(), "{:?}", sim.app.flash);
    W {
        sim,
        root,
        calls,
        _dir: dir,
    }
}

impl W {
    fn git(name: &str, changed: bool, lang: &str, style: &str, read_only: bool) -> W {
        let dir = sandbox(name).into_canonical().unwrap();
        let root = dir.join("repo");
        seed_write_repository(&root, changed);
        open_repo(dir, root, "git", name, lang, style, read_only, true)
    }

    fn jj(name: &str, lang: &str, confirm: bool, read_only: bool, colocated: bool) -> Option<W> {
        if !crate::vcs::jj::available() {
            return None;
        }
        let dir = sandbox(name).into_canonical().unwrap();
        let root = dir.join("repo");
        seed_jj_write_repository(&root, colocated);
        Some(open_repo(
            dir, root, "jj", name, lang, "relative", read_only, confirm,
        ))
    }

    fn key(&mut self, key: char) {
        self.sim.key(key);
        drain(&mut self.sim);
    }

    fn press(&mut self, code: KeyCode) {
        self.sim.press(code, KeyModifiers::NONE);
        drain(&mut self.sim);
    }

    /// Press without drawing in between, then land the action's job, so a refresh counter is read
    /// before any draw could have caused it.
    fn press_before_draw(&mut self, code: KeyCode) {
        handle_key(&mut self.sim.app, KeyEvent::new(code, KeyModifiers::NONE)).unwrap();
        for _ in 0..32 {
            self.sim.app.poll_projects();
            if !self.sim.app.project_jobs_pending() {
                return;
            }
            self.sim.app.wait_project_job();
        }
        panic!("VCS action did not finish");
    }

    fn type_text(&mut self, text: &str) {
        for c in text.chars() {
            self.key(c);
        }
    }

    fn row_values(&self) -> Vec<String> {
        let content = self.sim.app.project_content().unwrap();
        let ProjectContent::List(list) = content.as_ref() else {
            panic!("not a list")
        };
        list.rows.iter().map(|r| r.value.clone()).collect()
    }

    fn select(&mut self, relative: &str) {
        self.key('g');
        let want = self.root.join(relative).to_string_lossy().into_owned();
        let index = self
            .row_values()
            .iter()
            .position(|v| *v == want)
            .unwrap_or_else(|| panic!("missing {relative}; rows={:?}", self.rows()));
        for _ in 0..index {
            self.key('j');
        }
    }

    /// First cell of every row, with the repository root replaced by `<repo>`.
    fn rows(&self) -> Vec<String> {
        let content = self.sim.app.project_content().unwrap();
        let ProjectContent::List(list) = content.as_ref() else {
            panic!("not a list")
        };
        list.rows
            .iter()
            .map(|row| {
                row.cells[0]
                    .plain()
                    .replace(self.root.to_str().unwrap(), "<repo>")
            })
            .collect()
    }

    fn flash(&self) -> Option<String> {
        self.sim
            .app
            .flash
            .as_ref()
            .map(|f| f.replace(self.root.to_str().unwrap(), "<repo>"))
    }

    fn acts(&self) -> Vec<String> {
        self.calls.lock().unwrap().acts.clone()
    }

    fn counts(&self) -> (usize, usize) {
        let calls = self.calls.lock().unwrap();
        (calls.views, calls.chips)
    }

    fn run_git(&self, args: &[&str]) -> String {
        write_git_output(&self.root, args)
    }

    fn status(&self) -> String {
        // `--no-optional-locks`, the same etiquette as every background read in the app: this
        // status may run while the session's own `git commit` is parked in a hook holding
        // `index.lock` — without it the read itself can take the lock for an instant and make
        // the just-dispatched commit fail at startup.
        self.run_git(&["--no-optional-locks", "status", "--porcelain=v1"])
    }

    fn index(&self) -> Vec<u8> {
        std::fs::read(self.root.join(".git/index")).unwrap()
    }

    fn no_dialog(&self) {
        assert!(self.sim.app.project_dialog.is_none());
    }

    /// The confirmation text of the open dialog, resolved in the plugin's language; it is also
    /// drawn (the test backend leaves a blank cell after each wide glyph).
    fn confirmation(&self) -> String {
        let Some(ProjectDialog::Act {
            invocation,
            prompts,
            stage,
            ..
        }) = &self.sim.app.project_dialog
        else {
            panic!("missing confirmation")
        };
        let ProjectPrompt::Confirmation(prompt) = &prompts[*stage] else {
            panic!("not a confirmation")
        };
        let text = invocation.confirmation_text(prompt, self.sim.app.plugin_lang());
        let wide: String = text
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
        self.sim.see(&wide);
        text
    }

    /// (label, typed buffer) of the open text prompt.
    fn input(&self) -> (String, String) {
        let Some(ProjectDialog::Act {
            prompts,
            stage,
            buffer,
            ..
        }) = &self.sim.app.project_dialog
        else {
            panic!("missing input")
        };
        let ProjectPrompt::Text(prompt) = &prompts[*stage] else {
            panic!("not an input")
        };
        (
            prompt.label.get(self.sim.app.plugin_lang()).to_string(),
            buffer.clone(),
        )
    }
}

const FILES: [&str; 5] = [
    "a-modified",
    "b-new",
    "c-deleted",
    "d-renamed",
    "sub/日本語.txt",
];
const BASE_STATUS: &str =
    " M a-modified\n D c-deleted\nR  d-old -> d-renamed\n M sub/日本語.txt\n?? b-new\n";

/// How the repository shows a file's path in a flash or a row: relative to the launch directory's
/// parent (`relative`), otherwise the absolute path (the sandbox is outside HOME).
fn label(style: &str, file: &str) -> String {
    if style == "relative" {
        format!("repo/{file}")
    } else {
        format!("<repo>/{file}")
    }
}

/// The list rows of the changed fixture; `staged` files show `●` and their staged status letter.
/// The renamed file is staged from the start.
fn rows_with(style: &str, staged: &[&str]) -> Vec<String> {
    [
        ("a-modified", "M", "M"),
        ("b-new", "U", "A"),
        ("c-deleted", "D", "D"),
        ("d-renamed", "R", "R"),
        ("sub/日本語.txt", "M", "M"),
    ]
    .iter()
    .map(|(file, plain, on_staged)| {
        let on = *file == "d-renamed" || staged.contains(file);
        format!(
            "{}{} {}",
            if on { "● " } else { "  " },
            if on { on_staged } else { plain },
            label(style, file)
        )
    })
    .collect()
}

/// The rows once the renamed file's staged rename was unstaged: it falls apart into a staged
/// deletion of the old path and an untracked new path.
fn rows_after_rename_unstaged(style: &str) -> Vec<String> {
    vec![
        format!("  M {}", label(style, "a-modified")),
        format!("  U {}", label(style, "b-new")),
        format!("  D {}", label(style, "c-deleted")),
        format!("● D {}", label(style, "d-old")),
        format!("  U {}", label(style, "d-renamed")),
        format!("  M {}", label(style, "sub/日本語.txt")),
    ]
}

/// The rows after "unstage all" on the changed fixture: the rename falls apart and nothing is
/// staged any more (unlike a single-file unstage, which keeps the old path's deletion staged).
fn rows_after_unstage_all(style: &str) -> Vec<String> {
    let mut rows = rows_after_rename_unstaged(style);
    rows[3] = format!("  D {}", label(style, "d-old"));
    rows
}

const STATUS_AFTER_UNSTAGE_ALL: &str =
    " M a-modified\n D c-deleted\n D d-old\n M sub/日本語.txt\n?? b-new\n?? d-renamed\n";

fn status_after_stage(file: &str) -> &'static str {
    match file {
        "a-modified" => {
            "M  a-modified\n D c-deleted\nR  d-old -> d-renamed\n M sub/日本語.txt\n?? b-new\n"
        }
        "b-new" => {
            " M a-modified\nA  b-new\n D c-deleted\nR  d-old -> d-renamed\n M sub/日本語.txt\n"
        }
        "c-deleted" => {
            " M a-modified\nD  c-deleted\nR  d-old -> d-renamed\n M sub/日本語.txt\n?? b-new\n"
        }
        "d-renamed" => BASE_STATUS,
        "sub/日本語.txt" => {
            " M a-modified\n D c-deleted\nR  d-old -> d-renamed\nM  sub/日本語.txt\n?? b-new\n"
        }
        other => panic!("{other}"),
    }
}

const STATUS_AFTER_UNSTAGING_RENAME: &str =
    " M a-modified\n D c-deleted\nD  d-old\n M sub/日本語.txt\n?? b-new\n?? d-renamed\n";

fn write_tokens(app: &App) -> Vec<String> {
    crate::ui::status::hint_tokens(app)
        .into_iter()
        .filter(|t| {
            t.split_once(':').is_some_and(|(keys, _)| {
                matches!(keys, "s/S" | "u/U" | "S" | "U" | "s" | "u" | "x" | "c")
            })
        })
        .collect()
}

/// The write rows of the canvas help's first (screen) section, as (keys, words).
fn write_help(app: &App) -> Vec<(String, String)> {
    let section = crate::ops::help::sections_for(app, Context::Canvas)
        .into_iter()
        .next()
        .unwrap();
    section
        .rows
        .into_iter()
        .filter(|(keys, _)| {
            matches!(
                keys.as_str(),
                "s / S" | "u / U" | "S" | "U" | "x" | "c" | "d" | "s"
            )
        })
        .collect()
}

fn pairs(rows: &[(&str, &str)]) -> Vec<(String, String)> {
    rows.iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

fn strings(tokens: &[&str]) -> Vec<String> {
    tokens.iter().map(|s| s.to_string()).collect()
}

#[test]
fn vcs_port_write_stage_unstage_each_file_kind_style_and_language() {
    for lang in ["en", "ja"] {
        let (staged_word, unstaged_word) = if lang == "en" {
            ("Staged", "Unstaged")
        } else {
            ("ステージ", "アンステージ")
        };
        for style in ["relative", "home", "full"] {
            for file in FILES {
                let mut w = W::git("vcs_port_write_rows", true, lang, style, false);
                w.select(file);
                w.key('s');
                assert_eq!(
                    w.flash(),
                    Some(format!("{staged_word}: {}", label(style, file)))
                );
                assert_eq!(
                    w.status(),
                    status_after_stage(file),
                    "{lang} {style} {file}"
                );
                assert_eq!(w.rows(), rows_with(style, &[file]));
                w.select(file);
                w.key('u');
                assert_eq!(
                    w.flash(),
                    Some(format!("{unstaged_word}: {}", label(style, file)))
                );
                if file == "d-renamed" {
                    assert_eq!(w.status(), STATUS_AFTER_UNSTAGING_RENAME);
                    assert_eq!(w.rows(), rows_after_rename_unstaged(style));
                } else {
                    assert_eq!(w.status(), BASE_STATUS, "{lang} {style} {file}");
                    assert_eq!(w.rows(), rows_with(style, &[]));
                }
            }
        }
    }
}

#[test]
fn vcs_port_write_all_and_empty_guards_run_no_git() {
    for lang in ["en", "ja"] {
        let (all, unstaged_all, no_changes, nothing_staged) = if lang == "en" {
            (
                "Staged all (5)",
                "Unstaged all",
                "no changes",
                "nothing staged",
            )
        } else {
            (
                "全てステージ (5)",
                "全てアンステージ",
                "変更なし",
                "ステージ無し",
            )
        };
        // Empty repository: an index write would fail on the planted lock, so a guard flash
        // proves git never ran; index bytes and status stay as they were.
        let mut w = W::git("vcs_port_write_all", false, lang, "relative", false);
        std::fs::write(w.root.join(".git/index.lock"), "fixture lock").unwrap();
        let (index, status) = (w.index(), w.status());
        for (key, flash) in [
            ('S', no_changes),
            ('U', nothing_staged),
            ('U', nothing_staged),
        ] {
            w.key(key);
            assert_eq!(w.flash().as_deref(), Some(flash), "empty {lang} {key}");
            assert_eq!(w.index(), index);
            assert_eq!(w.status(), status);
            assert_eq!(
                std::fs::read_to_string(w.root.join(".git/index.lock")).unwrap(),
                "fixture lock"
            );
        }
        assert_eq!(
            w.rows(),
            [if lang == "en" {
                "  (no changes)"
            } else {
                "  (変更なし)"
            }]
        );
        // Changed repository: stage all, unstage all, then the second unstage has nothing to do.
        let mut w = W::git("vcs_port_write_all", true, lang, "relative", false);
        w.key('S');
        assert_eq!(w.flash().as_deref(), Some(all));
        assert_eq!(
            w.status(),
            "M  a-modified\nA  b-new\nD  c-deleted\nR  d-old -> d-renamed\nM  sub/日本語.txt\n"
        );
        assert_eq!(w.rows(), rows_with("relative", &FILES));
        w.key('U');
        assert_eq!(w.flash().as_deref(), Some(unstaged_all));
        assert_eq!(w.status(), STATUS_AFTER_UNSTAGE_ALL);
        assert_eq!(w.rows(), rows_after_unstage_all("relative"));
        w.key('U');
        assert_eq!(w.flash().as_deref(), Some(nothing_staged));
    }
}

#[test]
fn vcs_port_write_empty_list_keys_never_invoke_git_or_flash() {
    let mut w = W::git("vcs_port_write_empty_keys", false, "en", "relative", false);
    let (status, index, acts) = (w.status(), w.index(), w.acts());
    for key in ['s', 'u', 'x'] {
        w.key(key);
        assert_eq!(w.flash(), None, "{key}");
        w.no_dialog();
        assert_eq!(w.acts(), acts, "no action invocation means no git call");
        assert_eq!(w.index(), index);
        assert_eq!(w.status(), status);
        assert_eq!(w.rows(), ["  (no changes)"]);
    }
}

#[test]
fn vcs_port_write_discard_confirmation_success_and_failures() {
    for lang in ["en", "ja"] {
        let (discarded, failed) = if lang == "en" {
            ("Discarded", "failed")
        } else {
            ("破棄", "失敗")
        };
        for file in ["a-modified", "sub/日本語.txt"] {
            let mut w = W::git("vcs_port_write_discard", true, lang, "relative", false);
            w.select(file);
            w.key('x');
            let name = file.rsplit('/').next().unwrap();
            assert_eq!(
                w.confirmation(),
                if lang == "en" {
                    format!("Discard changes to {name} ?")
                } else {
                    format!("変更を破棄しますか: {name} ?")
                }
            );
            w.key('y');
            w.no_dialog();
            assert_eq!(
                std::fs::read_to_string(w.root.join(file)).unwrap(),
                format!("base {file}\n")
            );
            assert_eq!(
                w.flash(),
                Some(format!("{discarded}: {}", label("relative", file)))
            );
            let mut expected = rows_with("relative", &[]);
            expected.retain(|row| !row.ends_with(&label("relative", file)));
            assert_eq!(w.rows(), expected);
            assert_eq!(w.acts(), ["discard"]);
        }
        // An untracked file cannot be discarded: git's own message is flashed after "failed".
        let mut w = W::git("vcs_port_write_failures", true, lang, "relative", false);
        w.select("b-new");
        w.key('x');
        assert_eq!(
            w.confirmation(),
            if lang == "en" {
                "Discard changes to b-new ?"
            } else {
                "変更を破棄しますか: b-new ?"
            }
        );
        w.key('y');
        assert_eq!(
            w.flash(),
            Some(format!(
                "{failed}: error: pathspec '<repo>/b-new' did not match any file(s) known to git"
            ))
        );
        assert_eq!(w.rows(), rows_with("relative", &[]));
        std::fs::write(w.root.join(".git/index.lock"), "fixture lock").unwrap();
        for key in ['s', 'u', 'S', 'U'] {
            w.key(key);
            assert_eq!(
                w.flash(),
                Some(format!(
                    "{failed}: fatal: Unable to create '<repo>/.git/index.lock': File exists."
                )),
                "{lang} {key}"
            );
        }
    }
}

fn discard_cancel(key: KeyCode) {
    for lang in ["en", "ja"] {
        let mut w = W::git(
            "vcs_port_write_discard_cancel",
            true,
            lang,
            "relative",
            false,
        );
        w.select("a-modified");
        let (status, rows) = (w.status(), w.rows());
        w.key('x');
        assert_eq!(
            w.confirmation(),
            if lang == "en" {
                "Discard changes to a-modified ?"
            } else {
                "変更を破棄しますか: a-modified ?"
            }
        );
        w.press(key);
        assert_eq!(w.acts(), Vec::<String>::new());
        assert_eq!(w.status(), status);
        assert_eq!(w.rows(), rows);
        assert_eq!(
            w.flash().as_deref(),
            Some(if lang == "en" { "canceled" } else { "取消" })
        );
        w.no_dialog();
        assert_eq!(
            std::fs::read_to_string(w.root.join("a-modified")).unwrap(),
            "changed\n"
        );
    }
}

#[test]
fn vcs_port_write_discard_cancel_n_and_uppercase_n_flash_and_close() {
    discard_cancel(KeyCode::Char('n'));
    discard_cancel(KeyCode::Char('N'));
}

#[test]
fn vcs_port_write_discard_cancel_escape_flashes_and_closes() {
    discard_cancel(KeyCode::Esc);
}

#[test]
fn vcs_port_write_discard_enter_does_nothing_and_uppercase_y_confirms() {
    for lang in ["en", "ja"] {
        let mut w = W::git(
            "vcs_port_write_discard_confirm_keys",
            true,
            lang,
            "relative",
            false,
        );
        w.select("a-modified");
        let (status, rows) = (w.status(), w.rows());
        w.key('x');
        let text = w.confirmation();
        w.press_before_draw(KeyCode::Enter);
        drain(&mut w.sim);
        assert_eq!(w.confirmation(), text, "Enter leaves the confirmation open");
        assert_eq!(w.acts(), Vec::<String>::new());
        assert_eq!(w.status(), status);
        assert_eq!(w.rows(), rows);
        w.key('Y');
        w.no_dialog();
        assert_eq!(w.acts(), ["discard"]);
        assert_eq!(
            std::fs::read_to_string(w.root.join("a-modified")).unwrap(),
            "base a-modified\n"
        );
        let mut expected = rows_with("relative", &[]);
        expected.remove(0);
        assert_eq!(w.rows(), expected);
        assert_eq!(
            w.flash(),
            Some(format!(
                "{}: repo/a-modified",
                if lang == "en" { "Discarded" } else { "破棄" }
            ))
        );
    }
}

#[test]
fn vcs_port_write_commit_blank_success_and_failed_hook_keep_the_prompt() {
    for lang in ["en", "ja"] {
        let (label_text, empty, committed) = if lang == "en" {
            ("Commit message", "message is empty", "Committed")
        } else {
            (
                "コミットメッセージ",
                "メッセージが空です",
                "コミットしました",
            )
        };
        for fail in [false, true] {
            let mut w = W::git("vcs_port_write_commit", true, lang, "relative", false);
            w.key('S');
            w.key('c');
            assert_eq!(w.input(), (label_text.to_string(), String::new()));
            w.key(' ');
            w.press(KeyCode::Enter);
            assert_eq!(w.flash().as_deref(), Some(empty));
            w.no_dialog();
            assert_eq!(w.rows(), rows_with("relative", &FILES));
            assert_eq!(w.run_git(&["log", "--format=%s"]), "seed\n");
            if fail {
                std::fs::create_dir_all(w.root.join(".git/hooks")).unwrap();
                crate::test_support::write_executable(
                    &w.root.join(".git/hooks/pre-commit"),
                    "#!/bin/sh\necho fixture-hook-refused >&2\nexit 1\n",
                );
            }
            w.key('c');
            w.type_text("fixture commit 日本語");
            w.press(KeyCode::Enter);
            if fail {
                assert_eq!(w.flash().as_deref(), Some("fixture-hook-refused"));
                assert_eq!(
                    w.input(),
                    (label_text.to_string(), "fixture commit 日本語".to_string())
                );
                assert_eq!(w.run_git(&["log", "--format=%s"]), "seed\n");
                assert_eq!(w.rows(), rows_with("relative", &FILES));
            } else {
                assert_eq!(w.flash().as_deref(), Some(committed));
                w.no_dialog();
                assert_eq!(
                    w.run_git(&["log", "--format=%s"]),
                    "fixture commit 日本語\nseed\n"
                );
                assert_eq!(
                    w.rows(),
                    [if lang == "en" {
                        "  (no changes)"
                    } else {
                        "  (変更なし)"
                    }]
                );
            }
        }
    }
}

#[test]
fn vcs_port_write_commit_then_log_screen_shows_the_subject() {
    let mut w = W::git("vcs_port_write_commit_log", true, "en", "relative", false);
    w.select("a-modified");
    w.key('s');
    assert_eq!(w.status(), status_after_stage("a-modified"));
    w.key('c');
    w.type_text("subject-of-commit");
    w.press(KeyCode::Enter);
    assert_eq!(
        w.run_git(&["log", "--format=%s"]),
        "subject-of-commit\nseed\n"
    );
    // The committed rows (a-modified and the rename staged from the start) left the list; the
    // unstaged rows stay, and the canvas keeps its screen.
    assert_eq!(
        w.rows(),
        [
            "  U repo/b-new",
            "  D repo/c-deleted",
            "  M repo/sub/日本語.txt"
        ]
    );
    assert!(w.screen_is("changes"));
    // The log screen draws the new commit's subject.
    w.press(KeyCode::Tab);
    assert!(w.screen_is("log"));
    w.sim.see("subject-of-commit");
    w.sim.see("seed");
}

#[test]
fn vcs_port_write_stage_runs_in_the_background_and_stages_after_drain() {
    let mut w = W::git("vcs_port_write_stage_bg", true, "en", "relative", false);
    w.select("a-modified");
    w.sim.key('s');
    assert!(
        w.sim.app.project_write_running(),
        "the stage act is running right after the key"
    );
    drain(&mut w.sim);
    assert!(!w.sim.app.project_write_running());
    assert_eq!(w.status(), status_after_stage("a-modified"));
    assert_eq!(w.rows(), rows_with("relative", &["a-modified"]));
}

#[test]
fn vcs_port_write_read_only_refuses_every_git_write_before_prompt_or_dispatch() {
    for lang in ["en", "ja"] {
        let mut w = W::git("vcs_port_write_read_only", true, lang, "relative", true);
        let (status, index, rows) = (w.status(), w.index(), w.rows());
        for key in ['s', 'u', 'S', 'U', 'x', 'c'] {
            w.key(key);
            assert_eq!(
                w.flash().as_deref(),
                Some(if lang == "en" {
                    "this plugin is set to read-only ([plugins.<id>] read_only = true)"
                } else {
                    "このプラグインは読み取り専用です([plugins.<id>] read_only = true)"
                }),
                "{lang} {key}"
            );
            w.no_dialog();
            assert_eq!(w.acts(), Vec::<String>::new(), "{key}");
            assert_eq!(w.status(), status);
            assert_eq!(w.index(), index);
            assert_eq!(w.rows(), rows);
        }
    }
}

/// Cursor of the open text prompt.
fn input_cursor(w: &W) -> usize {
    let Some(ProjectDialog::Act { cursor, .. }) = &w.sim.app.project_dialog else {
        panic!("missing input")
    };
    *cursor
}

#[test]
fn vcs_port_write_submit_rechecks_read_only_and_keeps_the_typed_commit_message() {
    // Maps old dialog_submit_rechecks_the_guard_and_keeps_the_unexecuted_commit_message.
    let mut w = W::git(
        "vcs_port_write_submit_recheck",
        true,
        "en",
        "relative",
        false,
    );
    w.key('S');
    let (subjects, acts) = (commit_subjects_of(&w.root), w.acts());
    w.key('c');
    w.type_text("keep this message");
    assert_eq!(
        w.input(),
        (
            "Commit message".to_string(),
            "keep this message".to_string()
        )
    );
    // The plugin turns read-only after the prompt opened: the submit must re-check.
    w.sim.app.cfg.plugins.insert(
        "git".into(),
        toml::Table::from_iter([("read_only".into(), true.into())]),
    );
    w.press(KeyCode::Enter);
    assert_eq!(
        w.flash().as_deref(),
        Some(crate::i18n::tr(
            w.sim.app.lang,
            crate::i18n::Msg::PluginReadOnly
        ))
    );
    assert_eq!(
        w.input(),
        (
            "Commit message".to_string(),
            "keep this message".to_string()
        ),
        "the prompt is open again with the typed message"
    );
    assert_eq!(input_cursor(&w), "keep this message".chars().count());
    assert!(!w.sim.app.project_write_running());
    assert_eq!(w.acts(), acts, "no act ran");
    assert_eq!(commit_subjects_of(&w.root), subjects, "no commit landed");
    w.sim.see("keep this message");
}

#[test]
fn vcs_port_write_submit_refused_as_busy_keeps_the_typed_commit_message() {
    // Maps old refused_git_op_puts_the_typed_text_back. The real keys open the prompt and submit;
    // the runtime is put into the refusing state directly because a running write hides the `c`
    // key, so no key sequence can submit a prompt that was opened before another write began.
    for state in ["another_act_in_flight", "runtime_not_ready"] {
        let mut w = W::git("vcs_port_write_submit_busy", true, "en", "relative", false);
        w.key('S');
        let (subjects, acts) = (commit_subjects_of(&w.root), w.acts());
        w.key('c');
        w.type_text("a carefully written commit message");
        let key = w
            .sim
            .app
            .tab
            .project_navigation
            .as_ref()
            .unwrap()
            .project
            .clone();
        let set = |w: &mut W, busy: bool| {
            let runtime = w.sim.app.project_runtimes.get_mut(&key).unwrap();
            match state {
                "another_act_in_flight" => {
                    runtime.busy = busy;
                    runtime.acting = busy;
                }
                _ => runtime.ready = !busy,
            }
        };
        set(&mut w, true);
        w.sim.press(KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(
            w.flash().as_deref(),
            Some(crate::i18n::tr(
                w.sim.app.lang,
                crate::i18n::Msg::ProjectBusy
            )),
            "{state}: the flash says why nothing ran"
        );
        assert_eq!(
            w.input(),
            (
                "Commit message".to_string(),
                "a carefully written commit message".to_string()
            ),
            "{state}: the typed message is kept"
        );
        assert_eq!(
            input_cursor(&w),
            "a carefully written commit message".chars().count(),
            "{state}: the cursor stays at the end"
        );
        assert_eq!(w.acts(), acts, "{state}: no act ran");
        assert_eq!(commit_subjects_of(&w.root), subjects, "{state}");
        // Once the runtime is free, the same prompt submits the kept message.
        set(&mut w, false);
        w.press(KeyCode::Enter);
        w.no_dialog();
        assert_eq!(
            commit_subjects_of(&w.root)[0],
            "a carefully written commit message",
            "{state}"
        );
    }
}

fn commit_subjects_of(root: &std::path::Path) -> Vec<String> {
    crate::git::log(root, 200)
        .into_iter()
        .map(|c| c.summary)
        .collect()
}

#[test]
fn vcs_port_write_refresh_current_screen_tree_status_and_commit_chip() {
    let mut w = W::git("vcs_port_write_refresh", true, "en", "relative", false);
    w.select("a-modified");
    for key in ['s', 'u', 'S', 'U', 'x'] {
        let (views, _) = w.counts();
        w.sim.app.clear_project_tree_dirty();
        w.press_before_draw(KeyCode::Char(key));
        if key == 'x' {
            w.press_before_draw(KeyCode::Char('y'));
        }
        assert!(
            w.sim.app.project_tree_dirty(),
            "{key}: tree status not invalidated"
        );
        assert_eq!(w.counts().0, views + 1, "{key}: changes list not refreshed");
        drain(&mut w.sim);
    }
    assert_eq!(
        w.rows(),
        [
            "  U repo/b-new",
            "  D repo/c-deleted",
            "  D repo/d-old",
            "  U repo/d-renamed",
            "  M repo/sub/日本語.txt"
        ]
    );
    w.key('S');
    let (views, chips) = w.counts();
    w.sim.app.clear_project_tree_dirty();
    w.key('c');
    w.key('m');
    w.press_before_draw(KeyCode::Enter);
    assert!(w.sim.app.project_tree_dirty());
    assert_eq!(w.counts(), (views + 1, chips + 1));
    drain(&mut w.sim);
    assert_eq!(w.rows(), ["  (no changes)"]);
    assert_eq!(w.run_git(&["log", "-1", "--format=%s"]), "m\n");
}

#[test]
fn vcs_port_write_footer_and_help_write_hints_and_empty_list_exception() {
    for lang in ["en", "ja"] {
        let ja = lang == "ja";
        let changed_tokens = strings(if ja {
            &["s/S:ステージ(全)", "u/U:解除(全)", "x:破棄", "c:コミット"]
        } else {
            &[
                "s/S:stage(all)",
                "u/U:unstage(all)",
                "x:discard",
                "c:commit",
            ]
        });
        let changed_help = pairs(if ja {
            &[
                ("s / S", "ステージ / 全ステージ"),
                ("u / U", "アンステージ / 全アンステージ"),
                ("x", "ファイル破棄 (確認)"),
                ("c", "コミット (ステージ済み)"),
                ("d", "全変更まとめ diff (作業ツリー)"),
            ]
        } else {
            &[
                ("s / S", "stage / stage all"),
                ("u / U", "unstage / unstage all"),
                ("x", "discard file (confirm)"),
                ("c", "commit (staged)"),
                ("d", "diff of all changes (worktree)"),
            ]
        });
        // Decided difference: an empty list shows only the keys that act (no s / u / x).
        let empty_tokens = strings(if ja {
            &["S:ステージ(全)", "U:解除(全)", "c:コミット"]
        } else {
            &["S:stage(all)", "U:unstage(all)", "c:commit"]
        });
        let empty_help = pairs(if ja {
            &[
                ("S", "全ステージ"),
                ("U", "全アンステージ"),
                ("c", "コミット (ステージ済み)"),
                ("d", "全変更まとめ diff (作業ツリー)"),
            ]
        } else {
            &[
                ("S", "stage all"),
                ("U", "unstage all"),
                ("c", "commit (staged)"),
                ("d", "diff of all changes (worktree)"),
            ]
        });
        let mut w = W::git("vcs_port_write_hints", false, lang, "relative", false);
        assert_eq!(write_tokens(&w.sim.app), empty_tokens);
        assert_eq!(write_help(&w.sim.app), empty_help);
        let mut w2 = W::git("vcs_port_write_hints", true, lang, "relative", false);
        assert_eq!(write_tokens(&w2.sim.app), changed_tokens);
        assert_eq!(write_help(&w2.sim.app), changed_help);
        for file in ["d-renamed", "a-modified"] {
            w2.select(file);
            assert_eq!(write_tokens(&w2.sim.app), changed_tokens, "{file}");
            assert_eq!(write_help(&w2.sim.app), changed_help, "{file}");
        }
        w.key('j');
        assert_eq!(write_tokens(&w.sim.app), empty_tokens);
    }
}

/// The jj write hints. A read-only plugin shows no write hint (the key refuses), only the
/// read-only `d` row remains in the help.
fn jj_hints(w: &W, lang: &str, read_only: bool) {
    let ja = lang == "ja";
    let (tokens, help): (&[&str], &[(&str, &str)]) = match (ja, read_only) {
        (false, false) => (
            &["s:sync"],
            &[
                ("d", "diff of everything against @-"),
                ("s", "let jj snapshot the working copy (confirm)"),
            ],
        ),
        (true, false) => (
            &["s:同期"],
            &[
                ("d", "@- との差分をまとめて表示"),
                ("s", "jj に作業コピーを取り込ませる (確認)"),
            ],
        ),
        (false, true) => (&[], &[("d", "diff of everything against @-")]),
        (true, true) => (&[], &[("d", "@- との差分をまとめて表示")]),
    };
    assert_eq!(write_tokens(&w.sim.app), strings(tokens));
    assert_eq!(write_help(&w.sim.app), pairs(help));
}

fn jj_op_log(root: &std::path::Path) -> String {
    write_jj_state(root)[2].clone()
}

#[test]
fn vcs_port_write_jj_sync_confirmation_cancel_enter_and_uppercase_y() {
    for lang in ["en", "ja"] {
        let (text, canceled, done) = if lang == "en" {
            (
                "Let jj snapshot the working copy? (jj undo reverses it)",
                "canceled",
                "jj snapshotted the working copy",
            )
        } else {
            (
                "jj に作業コピーを取り込ませますか?(jj undo で戻せます)",
                "取消",
                "jj が作業コピーを取り込みました",
            )
        };
        for colocated in [false, true] {
            let Some(mut w) = W::jj("vcs_port_write_jj_confirm", lang, true, false, colocated)
            else {
                return;
            };
            jj_hints(&w, lang, false);
            let before = write_jj_state(&w.root);
            assert_eq!(before[0], "base\n", "reading changes must not snapshot");
            for cancel in [KeyCode::Char('n'), KeyCode::Char('N'), KeyCode::Esc] {
                w.key('s');
                assert_eq!(w.confirmation(), text);
                w.press(cancel);
                assert_eq!(w.acts(), Vec::<String>::new());
                assert_eq!(write_jj_state(&w.root), before);
                w.no_dialog();
                assert_eq!(w.flash().as_deref(), Some(canceled));
                assert_eq!(w.rows(), ["  M repo/a-modified"]);
            }
            w.key('s');
            assert_eq!(w.confirmation(), text);
            w.press(KeyCode::Enter);
            assert_eq!(w.confirmation(), text, "Enter leaves the confirmation open");
            assert_eq!(w.acts(), Vec::<String>::new());
            assert_eq!(write_jj_state(&w.root), before);
            let (views, chips) = w.counts();
            w.sim.app.clear_project_tree_dirty();
            w.press_before_draw(KeyCode::Char('Y'));
            assert!(w.sim.app.project_tree_dirty());
            assert_eq!(w.counts(), (views + 1, chips + 1));
            drain(&mut w.sim);
            w.no_dialog();
            assert_eq!(w.acts(), ["sync"]);
            assert_eq!(w.flash().as_deref(), Some(done));
            let after = write_jj_state(&w.root);
            assert_eq!(after[0], "changed\n");
            assert_ne!(after[2], before[2], "the operation log gained the snapshot");
            assert!(after[2].starts_with("snapshot working copy"));
            assert_eq!(w.rows(), ["  M repo/a-modified"]);
        }
    }
}

#[test]
fn vcs_port_write_jj_sync_lowercase_y_confirms_and_reports() {
    for lang in ["en", "ja"] {
        let (text, done) = if lang == "en" {
            (
                "Let jj snapshot the working copy? (jj undo reverses it)",
                "jj snapshotted the working copy",
            )
        } else {
            (
                "jj に作業コピーを取り込ませますか?(jj undo で戻せます)",
                "jj が作業コピーを取り込みました",
            )
        };
        let Some(mut w) = W::jj("vcs_port_write_jj_y", lang, true, false, false) else {
            return;
        };
        let before = write_jj_state(&w.root);
        w.key('s');
        assert_eq!(w.confirmation(), text);
        w.press(KeyCode::Char('y'));
        w.no_dialog();
        assert_eq!(w.acts(), ["sync"]);
        assert_eq!(w.flash().as_deref(), Some(done));
        let after = write_jj_state(&w.root);
        assert_eq!(after[0], "changed\n");
        assert_ne!(after[2], before[2], "the operation log gained the snapshot");
        assert!(after[2].starts_with("snapshot working copy"));
    }
}

#[test]
fn vcs_port_write_jj_sync_setting_false_snapshots_without_prompt() {
    for lang in ["en", "ja"] {
        for colocated in [false, true] {
            let Some(mut w) = W::jj(
                "vcs_port_write_jj_no_confirm",
                lang,
                false,
                false,
                colocated,
            ) else {
                return;
            };
            jj_hints(&w, lang, false);
            let before = write_jj_state(&w.root);
            assert_eq!(before[0], "base\n");
            w.key('s');
            w.no_dialog();
            assert_eq!(w.acts(), ["sync"]);
            assert_eq!(
                w.flash().as_deref(),
                Some(if lang == "en" {
                    "jj snapshotted the working copy"
                } else {
                    "jj が作業コピーを取り込みました"
                })
            );
            let after = write_jj_state(&w.root);
            assert_eq!(after[0], "changed\n");
            assert_ne!(after[2], before[2]);
            assert_eq!(w.rows(), ["  M repo/a-modified"]);
        }
    }
}

#[test]
fn vcs_port_write_jj_sync_read_only_refuses_before_prompt_or_snapshot() {
    for lang in ["en", "ja"] {
        for confirm in [false, true] {
            let Some(mut w) = W::jj("vcs_port_write_jj_read_only", lang, confirm, true, false)
            else {
                return;
            };
            jj_hints(&w, lang, true);
            let before = write_jj_state(&w.root);
            w.key('s');
            w.no_dialog();
            assert_eq!(w.acts(), Vec::<String>::new());
            assert_eq!(
                w.flash().as_deref(),
                Some(if lang == "en" {
                    "this plugin is set to read-only ([plugins.<id>] read_only = true)"
                } else {
                    "このプラグインは読み取り専用です([plugins.<id>] read_only = true)"
                })
            );
            assert_eq!(write_jj_state(&w.root), before);
            assert_eq!(w.rows(), ["  M repo/a-modified"]);
            let _ = jj_op_log(&w.root);
        }
    }
}

// ---- branches ----

fn current_branch(root: &std::path::Path) -> Option<String> {
    let repo = konoma_vcs_core::git2::Repository::open(root).unwrap();
    let name = repo.head().unwrap().shorthand().ok().map(str::to_string);
    name
}

/// (kind, value) of every visible row of the open list.
fn branch_values(sim: &Sim) -> Vec<(String, String)> {
    let content = sim.app.project_content().unwrap();
    let ProjectContent::List(list) = content.as_ref() else {
        panic!("list required")
    };
    sim.app
        .project_visible_rows()
        .iter()
        .map(|i| (list.rows[*i].kind.clone(), list.rows[*i].value.clone()))
        .collect()
}

fn branches(name: &str, lang: &str, read_only: bool) -> W {
    let dir = super::vcs_changes::branches_fixture(name)
        .into_canonical()
        .unwrap();
    let root = dir.to_path_buf();
    let mut w = open_repo(dir, root, "git", name, lang, "relative", read_only, true);
    w.key('b');
    assert_eq!(w.sim.app.project_frame().unwrap().key.screen, "branches");
    w
}

impl W {
    fn select_branch(&mut self, name: &str) {
        self.key('g');
        let index = branch_values(&self.sim)
            .iter()
            .position(|(_, v)| v == name)
            .unwrap();
        for _ in 0..index {
            self.key('j');
        }
        assert_eq!(
            branch_values(&self.sim)[self.sim.app.project_frame().unwrap().cursor].1,
            name
        );
    }

    fn screen_is(&self, screen: &str) -> bool {
        self.sim.app.project_frame().unwrap().key.screen == screen
    }

    fn has_ref(&self, name: &str) -> bool {
        crate::vcs::refs(&self.root).iter().any(|r| r.name == name)
    }
}

#[test]
fn vcs_port_branch_checkout_current_and_other_branch_reload_and_tree_status() {
    for lang in ["en", "ja"] {
        let mut w = branches("vcs_port_branch_checkout", lang, false);
        for name in ["main", "alpha", "日本語"] {
            w.select_branch(name);
            w.sim.app.clear_project_tree_dirty();
            w.press_before_draw(KeyCode::Enter);
            assert!(w.sim.app.project_tree_dirty(), "{name}");
            drain(&mut w.sim);
            assert_eq!(
                w.flash(),
                Some(format!(
                    "{}: {name}",
                    if lang == "en" {
                        "Switched to"
                    } else {
                        "切替"
                    }
                ))
            );
            assert_eq!(current_branch(&w.root).as_deref(), Some(name));
            assert!(w.screen_is("branches"), "stays on the branch screen");
            assert!(branch_values(&w.sim).contains(&("current".into(), name.into())));
            assert!(!w.sim.app.project_tree_dirty());
            assert_eq!(w.sim.app.git_branch(), Some(name));
        }
        w.select_branch("main");
        w.key('l');
        assert_eq!(
            w.flash(),
            Some(format!(
                "{}: main",
                if lang == "en" {
                    "Switched to"
                } else {
                    "切替"
                }
            ))
        );
        assert_eq!(current_branch(&w.root).as_deref(), Some("main"));
        // A real checkout conflict keeps git's exact failure text and the screen.
        run_git(&w.root, &["switch", "alpha"]);
        std::fs::write(w.root.join("a-staged.txt"), "branch change\n").unwrap();
        run_git(&w.root, &["add", "."]);
        run_git(&w.root, &["commit", "-qm", "alpha change"]);
        run_git(&w.root, &["switch", "main"]);
        std::fs::write(w.root.join("a-staged.txt"), "conflicting local change\n").unwrap();
        w.select_branch("alpha");
        w.press(KeyCode::Enter);
        assert!(w.flash().unwrap().contains("would be overwritten"));
        assert_eq!(current_branch(&w.root).as_deref(), Some("main"));
        assert!(w.screen_is("branches"));
    }
}

#[test]
fn vcs_port_branch_create_success_blank_and_failure_keeps_typed_input() {
    for lang in ["en", "ja"] {
        let label = if lang == "en" {
            "New branch"
        } else {
            "新規ブランチ"
        };
        let mut w = branches("vcs_port_branch_create", lang, false);
        w.key('c');
        assert_eq!(w.input(), (label.to_string(), String::new()));
        w.key(' ');
        w.key(' ');
        w.press(KeyCode::Enter);
        assert_eq!(
            w.flash().as_deref(),
            Some(if lang == "en" {
                "name is empty"
            } else {
                "名前が空です"
            })
        );
        w.no_dialog();
        w.key('c');
        assert_eq!(w.input(), (label.to_string(), String::new()));
        w.type_text("  alpha  ");
        w.press(KeyCode::Enter);
        assert!(w.flash().unwrap().contains("already exists"));
        assert_eq!(w.input(), (label.to_string(), "  alpha  ".to_string()));
        w.press(KeyCode::Esc);
        w.key('c');
        w.type_text("  created  ");
        w.press(KeyCode::Enter);
        assert_eq!(
            w.flash(),
            Some(format!(
                "{}: created",
                if lang == "en" {
                    "Created branch"
                } else {
                    "ブランチ作成"
                }
            ))
        );
        assert_eq!(current_branch(&w.root).as_deref(), Some("created"));
        assert!(w.screen_is("branches"));
        assert!(branch_values(&w.sim).contains(&("current".into(), "created".into())));
    }
}

#[test]
fn vcs_port_branch_delete_dialog_safe_force_cancel_and_current_guard() {
    for lang in ["en", "ja"] {
        let mut w = branches("vcs_port_branch_delete", lang, false);
        w.key('d');
        w.no_dialog();
        assert_eq!(
            w.flash().as_deref(),
            Some(if lang == "en" {
                "cannot delete the current branch"
            } else {
                "現在のブランチは削除できません"
            })
        );
        w.select_branch("alpha");
        w.key('d');
        assert_eq!(
            w.confirmation(),
            if lang == "en" {
                "Delete branch: alpha"
            } else {
                "ブランチ削除: alpha"
            }
        );
        w.key('n');
        assert_eq!(
            w.flash().as_deref(),
            Some(if lang == "en" { "canceled" } else { "取消" })
        );
        w.no_dialog();
        assert!(w.has_ref("alpha"));
        w.key('d');
        w.press(KeyCode::Esc);
        assert_eq!(
            w.flash().as_deref(),
            Some(if lang == "en" { "canceled" } else { "取消" })
        );
        w.no_dialog();
        assert!(w.has_ref("alpha"));
        w.key('d');
        w.key('y');
        assert_eq!(
            w.flash().as_deref(),
            Some(if lang == "en" {
                "Deleted branch: alpha"
            } else {
                "ブランチ削除: alpha"
            })
        );
        assert!(!w.has_ref("alpha"));
        assert!(
            !branch_values(&w.sim).iter().any(|(_, v)| v == "alpha"),
            "alpha left the list"
        );
        run_git(&w.root, &["switch", "日本語"]);
        std::fs::write(w.root.join("unmerged"), "only here\n").unwrap();
        run_git(&w.root, &["add", "."]);
        run_git(&w.root, &["commit", "-qm", "unmerged"]);
        run_git(&w.root, &["switch", "main"]);
        w.select_branch("日本語");
        w.key('d');
        w.key('y');
        assert!(w.flash().unwrap().contains("not fully merged"));
        assert!(w.has_ref("日本語"));
        w.key('d');
        w.key('!');
        assert_eq!(
            w.flash().as_deref(),
            Some(if lang == "en" {
                "Deleted branch: 日本語"
            } else {
                "ブランチ削除: 日本語"
            })
        );
        assert!(!w.has_ref("日本語"));
    }
}

#[test]
fn vcs_port_branch_read_only_guard_precedes_all_prompts_and_writes() {
    for lang in ["en", "ja"] {
        let mut w = branches("vcs_port_branch_read_only", lang, true);
        w.select_branch("alpha");
        w.sim.dont_see("checkout");
        for key in [KeyCode::Enter, KeyCode::Char('c'), KeyCode::Char('d')] {
            w.press(key);
            assert_eq!(
                w.flash().as_deref(),
                Some(if lang == "en" {
                    "this plugin is set to read-only ([plugins.<id>] read_only = true)"
                } else {
                    "このプラグインは読み取り専用です([plugins.<id>] read_only = true)"
                })
            );
            w.no_dialog();
            assert!(w.screen_is("branches"));
            assert!(!w.sim.app.project_write_running(), "{key:?} started a job");
            assert_eq!(current_branch(&w.root).as_deref(), Some("main"));
            assert_eq!(crate::vcs::refs(&w.root).len(), 3);
            assert_eq!(w.acts(), Vec::<String>::new());
        }
    }
}

#[test]
fn vcs_port_branch_empty_repository_allows_create_and_has_no_row_actions() {
    let dir = sandbox("vcs_port_branch_empty").into_canonical().unwrap();
    crate::test_support::init_git_repo(&dir);
    let root = dir.to_path_buf();
    let mut w = open_repo(
        dir,
        root,
        "git",
        "vcs_port_branch_empty",
        "en",
        "relative",
        false,
        true,
    );
    w.key('b');
    assert_eq!(branch_values(&w.sim), [("empty".into(), "".into())]);
    w.press(KeyCode::Enter);
    w.no_dialog();
    w.key('d');
    w.no_dialog();
    // Fact only: the old hub flashed "no copy target" with no branch selected; on the canvas
    // the `y` leader does not open and nothing is flashed. Whether that silence is intended
    // is undecided — this test only records what the new path does today.
    w.key('y');
    assert_eq!(w.sim.app.pending_leader, None);
    assert_eq!(w.flash(), None);
    w.no_dialog();
    w.key('c');
    assert!(w.sim.app.project_dialog.is_some());
    w.press(KeyCode::Enter);
    assert_eq!(w.flash().as_deref(), Some("name is empty"));
    w.no_dialog();
}

// ---- worktrees ----

fn worktrees(
    dir: crate::test_support::TmpDir,
    root: PathBuf,
    name: &str,
    lang: &str,
    read_only: bool,
) -> W {
    let mut w = open_repo(dir, root, "git", name, lang, "relative", read_only, true);
    w.key('w');
    assert!(w.screen_is("worktrees"));
    w
}

impl W {
    fn select_worktree(&mut self, path: &std::path::Path) {
        let index = branch_values(&self.sim)
            .iter()
            .position(|(_, v)| std::path::Path::new(v) == path)
            .unwrap_or_else(|| panic!("{path:?} in {:?}", branch_values(&self.sim)));
        self.key('g');
        for _ in 0..index {
            self.key('j');
        }
        assert_eq!(self.sim.app.project_frame().unwrap().cursor, index);
    }
}

#[test]
fn vcs_port_worktree_create_works_on_empty_filter_and_blank_does_not_reopen() {
    let dir = sandbox("vcs_port_worktree_create")
        .into_canonical()
        .unwrap();
    let (root, _, _) = super::vcs_log::seed_worktrees(&dir);
    let base = dir.to_path_buf();
    let mut w = worktrees(dir, root, "vcs_port_worktree_create", "en", false);
    w.key('/');
    w.type_text("no-match");
    w.press(KeyCode::Enter);
    w.key('c');
    assert_eq!(w.input().1, "");
    w.type_text("   ");
    w.press(KeyCode::Enter);
    assert_eq!(w.flash().as_deref(), Some("name is empty"));
    w.no_dialog();
    w.key('c');
    assert_eq!(w.input().1, "");
    w.type_text("from-empty");
    w.press(KeyCode::Enter);
    assert_eq!(w.sim.app.tab.root, base.join("from-empty"));
    assert!(w.sim.app.tab.root.join(".git").is_file());
    assert!(w.has_ref("from-empty"), "the worktree's branch was created");
}

#[test]
fn vcs_port_worktree_diff_empty_and_refused_rows_flash_without_push() {
    for lang in ["en", "ja"] {
        for which in 0..3 {
            let dir = sandbox("vcs_port_worktree_diff").into_canonical().unwrap();
            let (root, _, _) = super::vcs_log::seed_worktrees(&dir);
            let (bare, linked) = super::vcs_log::seed_bare_worktrees(&dir);
            let (open_root, path) = match which {
                0 => (root.clone(), root.clone()),
                1 => (root.clone(), dir.join("prunable")),
                _ => (linked.clone(), bare.clone()),
            };
            let mut w = worktrees(
                dir,
                open_root.clone(),
                "vcs_port_worktree_diff",
                lang,
                false,
            );
            w.select_worktree(&path);
            let cursor = w.sim.app.project_frame().unwrap().cursor;
            w.press(KeyCode::Char('d'));
            assert!(w.screen_is("worktrees"));
            assert_eq!(w.sim.app.project_frame().unwrap().cursor, cursor);
            let expected = match (lang, which) {
                ("en", 0) => "no changes",
                ("en", 1) => "worktree is missing (run `git worktree prune`)",
                ("en", _) => "bare repository — it has no checkout",
                ("ja", 0) => "変更なし",
                ("ja", 1) => {
                    "ワークツリーが見つかりません（`git worktree prune` を実行してください）"
                }
                _ => "ベアリポジトリ（実体のチェックアウトがありません）",
            };
            assert_eq!(w.flash().as_deref(), Some(expected));
            assert_eq!(w.sim.app.tab.root, open_root);
        }
    }
}

#[test]
fn vcs_port_worktree_read_only_blocks_create_before_prompt_but_navigation_and_diff_work() {
    for lang in ["en", "ja"] {
        for operation in ["enter", "diff"] {
            let dir = sandbox("vcs_port_worktree_ro").into_canonical().unwrap();
            let (root, linked, _) = super::vcs_log::seed_worktrees(&dir);
            std::fs::write(linked.join("file.txt"), "changed\n").unwrap();
            let mut w = worktrees(dir, root.clone(), "vcs_port_worktree_ro", lang, true);
            w.select_worktree(&linked);
            let count = crate::git::worktrees(&root).len();
            w.press(KeyCode::Char('c'));
            assert_eq!(
                w.flash().as_deref(),
                Some(if lang == "en" {
                    "this plugin is set to read-only ([plugins.<id>] read_only = true)"
                } else {
                    "このプラグインは読み取り専用です([plugins.<id>] read_only = true)"
                })
            );
            w.no_dialog();
            assert_eq!(w.acts(), Vec::<String>::new());
            assert_eq!(crate::git::worktrees(&root).len(), count);
            if operation == "enter" {
                w.press(KeyCode::Enter);
                assert_eq!(w.sim.app.tab.root, linked);
            } else {
                w.press(KeyCode::Char('d'));
                assert!(w.screen_is("worktree_diff"));
            }
        }
    }
}

/// `git_op_result_does_not_disturb_another_tab`'s `write:still-completes`: the discard whose
/// reply lands while a different tab is shown still did its work — the file reverts on disk —
/// and the tab in front is left alone. The asking tab picks its queued effect up when it is
/// shown again.
#[test]
fn vcs_port_write_discard_reply_landing_on_another_tab_still_reverts_the_file() {
    let mut w = W::git(
        "vcs_port_write_other_tab_discard",
        true,
        "en",
        "relative",
        false,
    );
    w.select("a-modified");
    w.key('x');
    // Confirm by a raw key so no poll applies the reply before the second tab is shown.
    handle_key(
        &mut w.sim.app,
        KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE),
    )
    .unwrap();
    // A tab rooted outside the repository when the reply lands: it does not reach it.
    w.sim.key('t');
    w.sim.app.tree_leave().unwrap();
    assert!(!w.sim.app.tab.root.starts_with(&w.root));
    w.sim.app.wait_project_job();
    assert_eq!(w.calls.lock().unwrap().acts, ["discard"]);
    assert_eq!(
        std::fs::read_to_string(w.root.join("a-modified")).unwrap(),
        "base a-modified\n",
        "the discard completes even though its reply landed on another tab"
    );
    assert_eq!(
        w.sim.app.flash, None,
        "the shown tab does not take the write's flash"
    );
    // Back on the asking tab the queued effect arrives: its flash reports the revert and its
    // refreshed list no longer carries the file.
    w.sim.key('[');
    w.sim.app.poll_projects();
    drain(&mut w.sim);
    assert!(
        w.sim.app.flash.is_some(),
        "the asking tab receives the write's flash"
    );
    assert!(
        !w.rows().iter().any(|row| row.contains("a-modified")),
        "the reverted file leaves the change list"
    );
}

/// `same_root_tab_is_refreshed_even_though_it_is_not_the_origin`: a second tab at the same root
/// that cached the change list before the write keeps the stale rows only until its own watcher
/// reports the index the stage wrote; that event dirties every cached view, so the tab that did
/// not ask still shows the latest list.
#[test]
fn vcs_port_write_stage_in_another_tab_revalidates_the_same_root_changes_screen() {
    let mut w = W::git("vcs_port_same_root_refresh", true, "en", "relative", false);
    // The second tab at the same root opens the change list before the write, so its rows are
    // the cached pre-write state rather than a fresh first fetch.
    w.sim.key('t');
    w.sim.app.tab.projects.wait_for_result();
    w.sim.app.poll_projects();
    w.sim.key('p');
    drain(&mut w.sim);
    assert!(w.sim.app.project_active(), "{:?}", w.sim.app.flash);
    assert!(w.sim.app.tab.projects.uses_injected_events());
    let other = w.sim.app.tab.id;
    assert_eq!(w.rows(), rows_with("relative", &[]));

    // Back on the first tab, stage the file.
    w.sim.key('[');
    assert_ne!(w.sim.app.tab.id, other);
    w.select("a-modified");
    w.key('s');
    assert_eq!(w.rows(), rows_with("relative", &["a-modified"]));

    // On the second tab the write did not arrive: its cached rows are still the stale ones.
    w.sim.key(']');
    assert_eq!(w.sim.app.tab.id, other);
    assert_eq!(
        w.rows(),
        rows_with("relative", &[]),
        "another tab's cached view does not move without its own invalidation"
    );
    // Its own watcher reports the index write the stage produced; every cached view is dirty
    // and the shown screen refetches.
    let gitdir = crate::git::git_dir(&w.root).unwrap();
    w.sim
        .app
        .tab
        .projects
        .inject_event(Ok(notify::Event::new(notify::EventKind::Modify(
            notify::event::ModifyKind::Any,
        ))
        .add_path(gitdir.join("index"))));
    drain(&mut w.sim);
    assert_eq!(
        w.rows(),
        rows_with("relative", &["a-modified"]),
        "the tab that did not ask still follows the latest change list"
    );
}

// The old-hub e2e ports live in their own file but reuse this file's fixtures (child module).
#[path = "vcs_hub_e2e_port.rs"]
mod vcs_hub_e2e_port;
