//! The worktrees screens of the project canvas, driven through the host's `p` entry only. Nothing
//! here opens the old hub or compares against it: rows, flashes, titles, help rows and tab state
//! are written out as values or derived from the fixture's data.
use super::vcs_changes::{drain, plugin_config};
use super::*;
use crate::app::project_screens::ProjectContent;
use crate::test_support::TmpDir;
use std::path::{Path, PathBuf};

struct Repo {
    root: PathBuf,
    linked: PathBuf,
    detached: PathBuf,
    missing: PathBuf,
    dir: TmpDir,
}

fn repo(name: &str) -> Repo {
    let dir = sandbox(name).into_canonical().unwrap();
    let (root, linked, detached) = super::vcs_log::seed_worktrees(&dir);
    Repo {
        missing: dir.join("prunable"),
        root,
        linked,
        detached,
        dir,
    }
}

fn open_cfg(root: &Path, cfg: Config, width: u16) -> Sim {
    let mut sim = Sim::with_config_sized(root, cfg, width, 20);
    sim.app.tab.projects.wait_for_result();
    sim.app.poll_projects();
    sim.key('p');
    drain(&mut sim);
    assert!(sim.app.project_active(), "{:?}", sim.app.flash);
    sim.key('w');
    drain(&mut sim);
    assert_eq!(screen(&sim), "worktrees");
    sim
}

fn open_with(root: &Path, name: &str, lang: &str, width: u16, worktree_dir: &str) -> Sim {
    let mut cfg = plugin_config("git", name, lang);
    cfg.git.worktree_dir = worktree_dir.into();
    let settings = cfg.plugins.entry("git".into()).or_default();
    settings.insert(
        "worktree_dir".into(),
        toml::Value::String(worktree_dir.into()),
    );
    settings.insert("read_only".into(), toml::Value::Boolean(false));
    open_cfg(root, cfg, width)
}

fn open(root: &Path, name: &str, lang: &str, width: u16) -> Sim {
    open_with(root, name, lang, width, "../")
}

fn screen(sim: &Sim) -> String {
    sim.app.project_frame().unwrap().key.screen.clone()
}

fn cursor(sim: &Sim) -> usize {
    sim.app.project_frame().unwrap().cursor
}

fn press(sim: &mut Sim, code: KeyCode, mods: KeyModifiers) {
    sim.press(code, mods);
    drain(sim);
}

fn key(sim: &mut Sim, c: char) {
    sim.key(c);
    drain(sim);
}

/// (kind, value, selectable) of every row of the list on screen.
fn rows(sim: &Sim) -> Vec<(String, String, bool)> {
    let content = sim.app.project_content().unwrap();
    let ProjectContent::List(list) = content.as_ref() else {
        panic!("list required")
    };
    list.rows
        .iter()
        .map(|r| (r.kind.clone(), r.value.clone(), r.selectable))
        .collect()
}

/// Values of the rows the filter leaves visible.
fn visible(sim: &Sim) -> Vec<(String, String, bool)> {
    let all = rows(sim);
    sim.app
        .project_visible_rows()
        .into_iter()
        .map(|i| all[i].clone())
        .collect()
}

/// Move the cursor onto the row whose value is `path`.
fn select(sim: &mut Sim, path: &Path) {
    let index = rows(sim)
        .iter()
        .position(|(_, v, _)| Path::new(v) == path)
        .unwrap_or_else(|| panic!("{path:?} in {:?}", rows(sim)));
    for _ in 0..50 {
        let at = cursor(sim);
        if at == index {
            return;
        }
        key(sim, if at < index { 'j' } else { 'k' });
    }
    panic!("row {index} not reached");
}

fn edge(sim: &mut Sim) -> String {
    let buf = sim
        .term
        .draw(|frame| ui::render(frame, &mut sim.app))
        .unwrap()
        .buffer
        .clone();
    let width = buf.area.width;
    crate::test_support::top_edge::row_text(&buf, 1, 0..width)
}

fn tabs_text(lang: &str) -> &'static str {
    if lang == "en" {
        " p:Changes  Log  Graph  b:Branches  w:Worktrees ┐"
    } else {
        " p:変更  ログ  グラフ  b:ブランチ  w:ワークツリー ┐"
    }
}

#[test]
fn vcs_port_worktree_top_edge_open_cursor_move_and_filter_rows() {
    let r = repo("vcs_port_wt_cells");
    for root in [&r.root, &r.linked] {
        for lang in ["en", "ja"] {
            for width in [80u16, 160] {
                let mut sim = open(root, "vcs_port_wt_cells", lang, width);
                let e = edge(&mut sim);
                assert!(e.starts_with("┌ Git worktrees  (4) ─"), "{e}");
                assert!(e.ends_with(&format!("─{}", tabs_text(lang))), "{e}");
                // The fixture lists main, detached, linked, prunable, in that order.
                let listed = rows(&sim);
                let kinds: Vec<_> = listed.iter().map(|(k, _, _)| k.as_str()).collect();
                // The last row is the placeholder shown when the filter hides everything.
                let mut expected_kinds =
                    ["current", "worktree", "worktree", "unavailable", "empty"];
                if root == &r.linked {
                    expected_kinds[0] = "worktree";
                    expected_kinds[2] = "current";
                }
                assert_eq!(kinds, expected_kinds);
                let paths: Vec<PathBuf> = listed[..4]
                    .iter()
                    .map(|(_, v, _)| PathBuf::from(v))
                    .collect();
                assert_eq!(
                    paths,
                    [
                        r.root.clone(),
                        r.detached.clone(),
                        r.linked.clone(),
                        r.missing.clone()
                    ]
                );
                // The cursor opens on the current worktree and `j` moves one row.
                let open_at = if root == &r.root { 0 } else { 2 };
                assert_eq!(cursor(&sim), open_at);
                key(&mut sim, 'j');
                assert_eq!(cursor(&sim), open_at + 1);
                for (query, hits) in [
                    ("日本語", vec![r.linked.clone()]),
                    ("LINKED", vec![r.linked.clone()]),
                ] {
                    let mut f = open(root, "vcs_port_wt_cells", lang, width);
                    f.key('/');
                    f.keys(query);
                    drain(&mut f);
                    let got: Vec<PathBuf> = visible(&f)
                        .iter()
                        .map(|(_, v, _)| PathBuf::from(v))
                        .collect();
                    assert_eq!(got, hits, "{query}");
                }
                let mut f = open(root, "vcs_port_wt_cells", lang, width);
                f.key('/');
                f.keys("no-match");
                drain(&mut f);
                let only = visible(&f);
                assert_eq!(only.len(), 1);
                assert_eq!(only[0].0, "empty");
                assert!(!only[0].2);
            }
        }
    }
}

#[test]
fn vcs_port_worktree_create_blank_and_git_failure_flash_without_reopen() {
    for lang in ["en", "ja"] {
        for (name, flash) in [
            (
                "   ",
                if lang == "en" {
                    "name is empty".to_string()
                } else {
                    "名前が空です".to_string()
                },
            ),
            ("main", String::new()),
        ] {
            let r = repo("vcs_port_wt_create_fail");
            let mut sim = open(&r.root, "vcs_port_wt_create_fail", lang, 160);
            key(&mut sim, 'c');
            {
                let Some(crate::app::project_screens::ProjectDialog::Act {
                    prompts, buffer, ..
                }) = &sim.app.project_dialog
                else {
                    panic!("text dialog required")
                };
                let crate::app::project_screens::ProjectPrompt::Text(prompt) = &prompts[0] else {
                    panic!("text input required")
                };
                assert_eq!(
                    prompt.label.get(sim.app.plugin_lang()),
                    if lang == "en" {
                        "New worktree: branch name"
                    } else {
                        "新規ワークツリーのブランチ名"
                    }
                );
                assert!(buffer.is_empty());
            }
            sim.keys(name);
            press(&mut sim, KeyCode::Enter, KeyModifiers::NONE);
            let got = sim.app.flash.clone().unwrap();
            if name == "main" {
                // git's own refusal, the same text in both languages.
                assert_eq!(
                    got,
                    format!("fatal: '{}/main/../main' already exists", r.dir.display())
                );
            } else {
                assert_eq!(got, flash);
            }
            assert!(sim.app.project_dialog.is_none());
            assert_eq!(sim.app.tab.root, r.root);
            assert_eq!(screen(&sim), "worktrees");
        }
    }
}

#[test]
fn vcs_port_worktree_create_switches_with_slash_existing_branch_and_custom_directory() {
    for lang in ["en", "ja"] {
        for (name, directory, existing) in [
            ("  feat/nested  ", "../", false),
            ("existing", "../placed", true),
        ] {
            let r = repo("vcs_port_wt_create_ok");
            if existing {
                super::vcs_log::worktree_git(&r.root, &["branch", name]);
            }
            let mut sim = open_with(&r.linked, "vcs_port_wt_create_ok", lang, 160, directory);
            key(&mut sim, 'c');
            sim.keys(name);
            press(&mut sim, KeyCode::Enter, KeyModifiers::NONE);
            let slug = name.trim().replace('/', "-");
            let target = r
                .root
                .join(directory)
                .join(&slug)
                .canonicalize()
                .expect("the worktree was created where the settings say");
            assert_eq!(target.file_name().unwrap(), slug.as_str());
            assert_eq!(sim.app.tab.root, target);
            assert_eq!(sim.app.tab.open_dir, target);
            let label = if lang == "en" {
                "Created worktree: "
            } else {
                "ワークツリー作成: "
            };
            assert_eq!(
                sim.app.flash.as_deref(),
                Some(format!("{label}{}", target.display()).as_str())
            );
            assert!(sim.app.project_dialog.is_none());
            // The host leaves the project canvas for the tree of the new root.
            assert!(!sim.app.project_active());
            assert_eq!(sim.app.tab.mode, Mode::Tree);
            assert!(target.join(".git").is_file());
        }
    }
}

#[test]
fn vcs_port_worktree_enter_each_kind_root_open_dir_and_flash_including_l() {
    let r = repo("vcs_port_wt_enter");
    let (bare, bare_linked) = super::vcs_log::seed_bare_worktrees(&r.dir);
    for lang in ["en", "ja"] {
        let en = lang == "en";
        let d = r.dir.display();
        for (root, path, kind, flash) in [
            (
                &r.root,
                r.linked.clone(),
                "worktree",
                format!("{}{d}/linked", if en { "Switched to: " } else { "切替: " }),
            ),
            (
                &r.root,
                r.detached.clone(),
                "worktree",
                format!(
                    "{}{d}/detached",
                    if en { "Switched to: " } else { "切替: " }
                ),
            ),
            (
                &r.root,
                r.root.clone(),
                "current",
                if en {
                    "already in this worktree".to_string()
                } else {
                    "既にこのワークツリーにいます".to_string()
                },
            ),
            (
                &r.root,
                r.missing.clone(),
                "unavailable",
                if en {
                    "worktree is missing (run `git worktree prune`)".to_string()
                } else {
                    "ワークツリーが見つかりません（`git worktree prune` を実行してください）"
                        .to_string()
                },
            ),
            (
                &bare_linked,
                bare.clone(),
                "bare",
                if en {
                    "bare repository — it has no checkout".to_string()
                } else {
                    "ベアリポジトリ（実体のチェックアウトがありません）".to_string()
                },
            ),
        ] {
            for code in [KeyCode::Enter, KeyCode::Char('l')] {
                let mut sim = open(root, "vcs_port_wt_enter", lang, 160);
                select(&mut sim, &path);
                press(&mut sim, code, KeyModifiers::NONE);
                assert_eq!(sim.app.flash.as_deref(), Some(flash.as_str()), "{kind}");
                if kind == "worktree" {
                    assert_eq!(sim.app.tab.root, path, "{kind}");
                    assert_eq!(sim.app.tab.open_dir, path, "{kind}");
                    assert!(!sim.app.project_active());
                    assert_eq!(sim.app.tab.mode, Mode::Tree, "{kind}");
                    assert!(!sim.app.tab.entries.is_empty(), "{kind}");
                } else {
                    assert_eq!(&sim.app.tab.root, root, "{kind}");
                    assert_eq!(&sim.app.tab.open_dir, root, "{kind}");
                    assert!(sim.app.project_active());
                    assert_eq!(screen(&sim), "worktrees");
                }
            }
        }
    }
}

#[test]
fn vcs_port_host_only_worktree_ctrl_t_opens_new_tab_keeps_original_and_file_target_previews() {
    let r = repo("vcs_port_wt_ctrl_t");
    let mut sim = open(&r.root, "vcs_port_wt_ctrl_t", "en", 160);
    select(&mut sim, &r.linked);
    let at = cursor(&sim);
    press(&mut sim, KeyCode::Char('t'), KeyModifiers::CONTROL);
    assert_eq!(sim.app.tab_count(), 2);
    assert_eq!(sim.app.tab.root, r.linked);
    assert_eq!(sim.app.tab.open_dir, r.linked);
    assert_eq!(sim.app.tab.mode, Mode::Tree);
    assert!(!sim.app.tab.entries.is_empty());
    assert!(
        !sim.app.project_active(),
        "the new tab has no project canvas"
    );
    assert_eq!(sim.app.flash, None);
    sim.app.tab_goto(0);
    drain(&mut sim);
    assert_eq!(sim.app.tab.root, r.root);
    assert_eq!(screen(&sim), "worktrees");
    assert_eq!(cursor(&sim), at);
    // A changes row names a file: Ctrl-t previews it in the new tab.
    std::fs::write(r.root.join("file.txt"), "changed\n").unwrap();
    let cfg = plugin_config("git", "vcs_port_wt_ctrl_t_file", "en");
    let mut file = Sim::with_config_sized(&r.root, cfg, 160, 20);
    file.app.tab.projects.wait_for_result();
    file.app.poll_projects();
    file.key('p');
    drain(&mut file);
    press(&mut file, KeyCode::Char('t'), KeyModifiers::CONTROL);
    assert_eq!(file.app.tab_count(), 2);
    assert_eq!(file.app.tab.mode, Mode::Preview);
    assert_eq!(
        file.app.tab.preview_path.as_deref(),
        Some(r.root.join("file.txt").as_path())
    );
}

#[test]
fn vcs_port_worktree_filter_ignores_control_chords_and_keeps_shift_text() {
    let r = repo("vcs_port_wt_filter");
    for lang in ["en", "ja"] {
        let mut sim = open(&r.root, "vcs_port_wt_filter", lang, 160);
        select(&mut sim, &r.linked);
        sim.key('/');
        for letter in ['t', 'c', 'x', 'w', 'q', 'a', 'd'] {
            press(&mut sim, KeyCode::Char(letter), KeyModifiers::CONTROL);
            let frame = sim.app.project_frame().unwrap();
            assert_eq!(frame.query, "", "Ctrl-{letter} is not text");
            assert!(frame.filtering);
            assert_eq!(frame.key.screen, "worktrees");
            assert_eq!(sim.app.tab_count(), 1);
            assert_eq!(sim.app.tab.root, r.root);
            assert!(sim.app.project_dialog.is_none());
        }
        for (code, mods) in [
            (KeyCode::Char('日'), KeyModifiers::NONE),
            (KeyCode::Char('Q'), KeyModifiers::SHIFT),
        ] {
            sim.press(code, mods);
        }
        assert_eq!(sim.app.project_frame().unwrap().query, "日Q");
        for (code, mods) in [
            (KeyCode::Char('x'), KeyModifiers::ALT),
            (KeyCode::Char('\n'), KeyModifiers::NONE),
            (KeyCode::Char('\0'), KeyModifiers::NONE),
        ] {
            sim.press(code, mods);
            assert_eq!(sim.app.project_frame().unwrap().query, "日Q");
        }
        sim.press(KeyCode::Backspace, KeyModifiers::NONE);
        assert_eq!(sim.app.project_frame().unwrap().query, "日");
        sim.enter();
        assert!(!sim.app.project_frame().unwrap().filtering);
        assert_eq!(sim.app.project_frame().unwrap().query, "日");
        sim.key('/');
        sim.esc();
        assert!(sim.app.project_frame().unwrap().query.is_empty());
        assert!(!sim.app.project_frame().unwrap().filtering);
    }
}

#[test]
fn vcs_port_host_only_worktree_switch_vanished_after_plugin_check_keeps_screen_and_root() {
    use crate::plugins::{
        test_plugins,
        test_projects::{ObservedVcs, VcsCalls},
    };
    use std::sync::{Arc, Mutex};
    for lang in ["en", "ja"] {
        let r = repo("vcs_port_wt_vanished");
        let calls = Arc::new(Mutex::new(VcsCalls {
            remove_switch_target: true,
            ..Default::default()
        }));
        let token = test_plugins::register_replacement(
            &format!("vcs_port_switch_vanished_{lang}"),
            Arc::new(ObservedVcs {
                plugin: Arc::new(konoma_vcs::Git),
                calls: calls.clone(),
            }),
        );
        let mut cfg = test_plugins::marker_config(&token);
        cfg.ui.lang = lang.into();
        let mut sim = open_cfg(&r.root, cfg, 160);
        select(&mut sim, &r.linked);
        let root = sim.app.tab.root.clone();
        let open_dir = sim.app.tab.open_dir.clone();
        let frame = sim.app.project_frame().unwrap().clone();
        sim.enter();
        drain(&mut sim);
        assert!(!r.linked.exists());
        assert!(calls
            .lock()
            .unwrap()
            .acts
            .iter()
            .any(|a| a == "worktree_switch"));
        assert_eq!(sim.app.tab.root, root);
        assert_eq!(sim.app.tab.open_dir, open_dir);
        assert!(
            sim.app.project_active(),
            "a refused root switch must keep the worktree screen"
        );
        let after = sim.app.project_frame().unwrap();
        assert_eq!(after.key, frame.key);
        assert_eq!(
            (after.cursor, after.scroll, after.query.as_str()),
            (frame.cursor, frame.scroll, frame.query.as_str())
        );
        assert_eq!(
            sim.app.flash,
            Some(format!(
                "{}{}",
                crate::i18n::tr(sim.app.lang, crate::i18n::Msg::NotFound),
                r.linked.display()
            ))
        );
    }
}

#[test]
fn vcs_port_host_only_worktree_ctrl_t_without_target_is_inert_and_unlisted() {
    let r = repo("vcs_port_wt_no_target");
    let (bare, bare_linked) = super::vcs_log::seed_bare_worktrees(&r.dir);
    for (root, path) in [
        (&r.root, r.root.clone()),
        (&r.root, r.missing.clone()),
        (&bare_linked, bare),
    ] {
        let mut sim = open(root, "vcs_port_wt_no_target", "en", 160);
        select(&mut sim, &path);
        let at = cursor(&sim);
        assert!(!sim.app.project_controls().unwrap().contains("Ctrl-t"));
        assert!(!crate::ops::help::sections(&sim.app)
            .iter()
            .flat_map(|s| &s.rows)
            .any(|(k, _)| k.contains("Ctrl-t")));
        let flash = sim.app.flash.clone();
        press(&mut sim, KeyCode::Char('t'), KeyModifiers::CONTROL);
        assert_eq!(sim.app.tab_count(), 1);
        assert_eq!(&sim.app.tab.root, root);
        assert_eq!(cursor(&sim), at);
        assert_eq!(sim.app.flash, flash);
    }
}

#[test]
fn vcs_port_worktree_diff_title_body_layout_motion_and_back() {
    let r = repo("vcs_port_wt_diff");
    let body = |head: &str| {
        (0..70)
            .map(|i| format!("{head} {i} {}\n", "long ".repeat(30)))
            .collect::<String>()
    };
    std::fs::write(r.linked.join("file.txt"), body("line")).unwrap();
    super::vcs_log::commit_worktree(&r.linked);
    std::fs::write(r.root.join("file.txt"), body("uncommitted")).unwrap();
    // An uncommitted (untracked) file in the linked worktree joins its committed diff.
    std::fs::write(r.linked.join("dirty.txt"), "dirty\n").unwrap();
    for lang in ["en", "ja"] {
        for width in [80u16, 160] {
            for (path, head, title) in [
                (
                    &r.linked,
                    "line",
                    format!(" 日本語 ← main  {} ", r.linked.display()),
                ),
                (
                    &r.root,
                    "uncommitted",
                    format!(
                        " main  {}  ({}) ",
                        r.root.display(),
                        if lang == "en" {
                            "Uncommitted changes"
                        } else {
                            "未コミットの変更"
                        }
                    ),
                ),
            ] {
                let mut sim = open(&r.root, "vcs_port_wt_diff", lang, width);
                select(&mut sim, path);
                let at = cursor(&sim);
                key(&mut sim, 'd');
                assert_eq!(screen(&sim), "worktree_diff");
                {
                    let content = sim.app.project_content().unwrap();
                    let ProjectContent::Diff(diff) = content.as_ref() else {
                        panic!("diff required")
                    };
                    assert!(diff.header.is_empty(), "the title is not a body line");
                    assert_eq!(
                        diff.title.as_ref().map(|t| t.to_string()),
                        Some(title.clone())
                    );
                    // One file: its heading, the removed `base` line, then the 70 added lines
                    // (the linked case adds the uncommitted file's heading and added line).
                    assert_eq!(diff.lines.len(), if path == &r.linked { 74 } else { 72 });
                    if path == &r.linked {
                        let dirty = diff
                            .lines
                            .iter()
                            .position(|l| l.text.trim_end() == "dirty.txt")
                            .expect("the uncommitted file is in the diff");
                        let added = &diff.lines[dirty + 1];
                        assert_eq!(
                            (added.old_no, added.new_no, added.text.trim_end()),
                            (None, Some(1), "dirty")
                        );
                    }
                    let file = diff
                        .lines
                        .iter()
                        .position(|l| l.text.trim_end() == "file.txt")
                        .unwrap();
                    assert_eq!(
                        (
                            diff.lines[file + 1].old_no,
                            diff.lines[file + 1].text.trim_end()
                        ),
                        (Some(1), "base")
                    );
                    for (i, line) in diff.lines[file + 2..].iter().enumerate() {
                        assert_eq!(line.new_no, Some(i as u32 + 1));
                        assert_eq!(
                            line.text.trim_end(),
                            format!("{head} {i} {}", "long ".repeat(30)).trim_end()
                        );
                    }
                }
                let e = edge(&mut sim);
                let full = format!("┌{title}");
                // The title runs up to the filler; a title wider than the frame is clipped and
                // its last cell gives way to `┐`.
                let head_of = |e: &str| -> String {
                    e.chars().take_while(|c| *c != '─' && *c != '┐').collect()
                };
                let head = head_of(&e);
                assert!(full.starts_with(&head), "{e:?} vs {full:?}");
                if width == 160 {
                    assert_eq!(head, full);
                } else {
                    assert!(head.chars().count() > 60, "{e:?}");
                }
                for k in ['j', 'l', 's'] {
                    key(&mut sim, k);
                    assert_eq!(screen(&sim), "worktree_diff", "{k}");
                    let now = head_of(&edge(&mut sim));
                    if k == 's' && width == 160 {
                        // The layout key adds the side-by-side mark after the title.
                        assert_eq!(now, format!("{head}⇆ "), "{k}");
                    } else if k == 's' {
                        assert!(full.starts_with(&now), "{k}");
                    } else {
                        assert_eq!(now, head, "{k}");
                    }
                }
                key(&mut sim, 'q');
                assert_eq!(screen(&sim), "worktrees");
                assert_eq!(cursor(&sim), at);
            }
        }
    }
}

#[test]
fn vcs_port_worktree_help_rows_in_both_languages_with_only_target_exception() {
    let r = repo("vcs_port_wt_help");
    for lang in ["en", "ja"] {
        for path in [&r.root, &r.linked] {
            let mut sim = open(&r.root, "vcs_port_wt_help", lang, 160);
            select(&mut sim, path);
            let en = lang == "en";
            let mut expected = vec![
                (
                    "l / Enter".to_string(),
                    if en {
                        "switch to it"
                    } else {
                        "切り替える"
                    }
                    .to_string(),
                ),
                (
                    "c".to_string(),
                    if en {
                        "new worktree (new or existing branch)"
                    } else {
                        "新規ワークツリー（新規/既存ブランチ）"
                    }
                    .to_string(),
                ),
                (
                    "d".to_string(),
                    if en {
                        "diff since base (or uncommitted)"
                    } else {
                        "base からの diff（無ければ未コミットのみ）"
                    }
                    .to_string(),
                ),
                (
                    "/".to_string(),
                    if en {
                        "filter by name"
                    } else {
                        "名前で絞り込み"
                    }
                    .to_string(),
                ),
            ];
            if path == &r.linked {
                expected.push((
                    "Ctrl-t".to_string(),
                    if en {
                        "open the entry under the cursor in a new tab"
                    } else {
                        "カーソル下のエントリを別タブで開く"
                    }
                    .to_string(),
                ));
            }
            expected.sort();
            let sections = crate::ops::help::sections(&sim.app);
            let mut actual: Vec<_> = sections
                .iter()
                .flat_map(|s| &s.rows)
                .filter(|(k, _)| ["l / Enter", "c", "Ctrl-t", "d", "/"].contains(&k.as_str()))
                .cloned()
                .collect();
            actual.sort();
            assert_eq!(actual, expected);
            assert_eq!(
                sections[0].title,
                if en {
                    "Git worktrees (w)"
                } else {
                    "Git ワークツリー (w)"
                }
            );
            assert!(!sections.iter().flat_map(|s| &s.rows).any(|(k, _)| k == "V"));
        }
    }
}
