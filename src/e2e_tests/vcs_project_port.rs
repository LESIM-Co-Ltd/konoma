//! Literal B/C expectations through standard discovery, p, and the drawn project screen.
use super::vcs_changes::drain;
use super::*;
use crate::i18n::{tr, Msg};
use crate::keymap::{Context, LeaderId};
use konoma_host_core::clipboard::{clear_test_clipboard, get_test_clipboard};
use konoma_plugin::host::project::ProjectContent;

include!("vcs_project_style_cases.rs");
include!("vcs_project_remaining_cases.rs");
include!("vcs_project_finish_cases.rs");
include!("vcs_project_gap_cases.rs");

fn isolated(name: &str) -> bool {
    super::vcs_host_only::isolate_test("vcs_project_port", name)
}

fn config(id: &str, lang: &str) -> Config {
    let mut cfg = Config::default();
    cfg.ui.lang = lang.into();
    cfg.ui.tabbar = "hidden".into();
    cfg.ui.statusbar = "split".into();
    cfg.plugins
        .entry(if id == "git" { "jj" } else { "git" }.into())
        .or_default()
        .insert("enabled".into(), toml::Value::Boolean(false));
    cfg
}

fn open(root: &std::path::Path, cfg: Config, size: (u16, u16)) -> Sim {
    let mut sim = Sim::with_config_sized(root, cfg, size.0, size.1);
    sim.app.tab.projects.wait_for_result();
    sim.app.poll_projects();
    let (_, legacy) = App::count_legacy_hub_opens(|| sim.key('p'));
    drain(&mut sim);
    assert_eq!(legacy, 0);
    assert!(sim.app.project_active());
    sim
}

fn press(sim: &mut Sim, code: KeyCode) {
    sim.press(code, KeyModifiers::NONE);
    drain(sim);
}

fn keys(sim: &mut Sim, text: &str) {
    for c in text.chars() {
        press(sim, KeyCode::Char(c));
    }
}

fn screen(sim: &mut Sim, name: &str) {
    for _ in 0..5 {
        if sim.app.project_frame().unwrap().key.screen == name {
            return;
        }
        press(sim, KeyCode::Tab);
    }
    panic!("screen not declared: {name}");
}

#[test]
fn vcs_port_copy_empty_and_unbound_prefix_have_no_hints_or_effects() {
    if isolated("vcs_port_copy_empty_and_unbound_prefix_have_no_hints_or_effects") {
        return;
    }
    let root = super::vcs_log::repo(0);
    let mut sim = open(&root, config("git", "en"), (100, 12));
    for name in ["log", "changes", "worktrees"] {
        screen(&mut sim, name);
        clear_test_clipboard();
        assert!(!crate::ops::menu::opens(
            &sim.app,
            Context::Canvas,
            LeaderId::Copy
        ));
        assert!(!crate::ops::footer::tokens(&sim.app, Context::Canvas)
            .join(" ")
            .contains("y:"));
        assert!(
            crate::ui::help::leader_section(&sim.app, Context::Canvas, LeaderId::Copy, "y")
                .is_none()
        );
        keys(&mut sim, "y");
        assert_eq!(sim.app.pending_leader, None);
        assert_eq!(sim.app.flash, None);
        assert_eq!(get_test_clipboard(), None);
    }
    let root = super::vcs_log::repo(1);
    let mut cfg = config("git", "en");
    cfg.keys
        .surfaces
        .insert("canvas".into(), [("y".into(), "noop".into())].into());
    let mut sim = open(&root, cfg, (100, 12));
    screen(&mut sim, "log");
    assert!(crate::ops::menu::opens(
        &sim.app,
        Context::Canvas,
        LeaderId::Copy
    ));
    keys(&mut sim, "y");
    assert_eq!(sim.app.pending_leader, None);
    assert!(!crate::ops::footer::tokens(&sim.app, Context::Canvas)
        .join(" ")
        .contains("y:"));
    let title = tr(sim.app.lang, Msg::CopyHint);
    assert!(crate::ops::help::sections_for(&sim.app, Context::Canvas)
        .iter()
        .all(|s| s.title != title));
}

#[test]
fn vcs_port_copy_menu_literal_labels_help_footer_and_space_membership() {
    if isolated("vcs_port_copy_menu_literal_labels_help_footer_and_space_membership") {
        return;
    }
    use crate::ops::menu::{menu, View};
    let root = super::vcs_log::detail_repo();
    for lang in ["en", "ja"] {
        for detail in [false, true] {
            let mut sim = open(&root, config("git", lang), (180, 14));
            screen(&mut sim, "log");
            if detail {
                press(&mut sim, KeyCode::Enter);
            }
            let copy = menu(&sim.app, Context::Canvas, LeaderId::Copy, View::Open).unwrap();
            assert_eq!(
                copy.items
                    .iter()
                    .map(|i| (i.key.code, i.label.clone()))
                    .collect::<Vec<_>>(),
                [
                    ('s', Msg::WkShortHash),
                    ('h', Msg::WkFullHash),
                    ('t', Msg::WkSubject),
                    ('m', Msg::WkMessage),
                    ('a', Msg::WkAuthor),
                    ('d', Msg::WkDate)
                ]
                .map(|(key, msg)| (KeyCode::Char(key), tr(sim.app.lang, msg).to_string()))
            );
            assert_eq!(copy.title, tr(sim.app.lang, Msg::CopyHint));
            let expected =
                crate::ui::help::leader_section(&sim.app, Context::Canvas, LeaderId::Copy, "y")
                    .unwrap();
            assert!(crate::ops::help::sections_for(&sim.app, Context::Canvas)
                .iter()
                .any(|s| s.title == expected.title && s.rows == expected.rows));
            assert!(crate::ops::footer::tokens(&sim.app, Context::Canvas)
                .contains(&format!("y:{}", tr(sim.app.lang, Msg::CopyHint))));
            let space = menu(&sim.app, Context::Canvas, LeaderId::Actions, View::Open).unwrap();
            assert!(space
                .items
                .iter()
                .all(|i| copy.items.iter().all(|c| c.action != i.action)));
            let footer = crate::ops::footer::render_tokens(
                &sim.app,
                Context::Canvas,
                &crate::ops::footer::plan(&sim.app, Context::Canvas),
            );
            assert!(footer.iter().all(|t| t
                .actions
                .iter()
                .all(|a| copy.items.iter().all(|c| &c.action != a))));
            keys(&mut sim, "y");
            assert_eq!(sim.app.pending_leader, Some(LeaderId::Copy));
            let expected = konoma_widgets::footer_line::which_key_spans(
                &copy.title,
                &copy
                    .items
                    .iter()
                    .map(|i| (konoma_host_core::help::help_key(i.key), i.label.clone()))
                    .collect::<Vec<_>>(),
                180,
            );
            use ratatui::widgets::Widget;
            let mut row = ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, 180, 1));
            ratatui::widgets::Paragraph::new(ratatui::text::Line::from(expected))
                .render(row.area, &mut row);
            let frame = sim.project_frame();
            for x in 0..180 {
                assert_eq!(frame[(x, 13)], row[(x, 0)], "copy menu cell {x}");
            }
        }
    }
}

#[test]
fn vcs_port_log_and_changes_help_filter_marks_and_footer_priority() {
    if isolated("vcs_port_log_and_changes_help_filter_marks_and_footer_priority") {
        return;
    }
    let root = super::vcs_changes::git_repo("vcs_port_shared_controls", false);
    std::fs::write(root.join("file.txt"), "changed\n").unwrap();
    for lang in ["en", "ja"] {
        let mut sim = open(&root, config("git", lang), (160, 20));
        for (name, query) in [("changes", "FILE.TXT"), ("log", "SEED")] {
            screen(&mut sim, name);
            let prefix = if lang == "en" {
                "Tab/Shift-Tab:screens  j/k:move  /:filter  q:back  ?:help"
            } else {
                "Tab/Shift-Tab:画面  j/k:移動  /:絞り込み  q:戻る  ?:ヘルプ"
            };
            let label = if name == "changes" {
                if lang == "en" {
                    "diff"
                } else {
                    "差分"
                }
            } else if lang == "en" {
                "detail"
            } else {
                "詳細"
            };
            let target = if name == "changes" {
                format!("  Ctrl-t:{}", tr(sim.app.lang, Msg::HintNewTab))
            } else {
                String::new()
            };
            let copy = if name == "log" {
                format!("  y:{}", tr(sim.app.lang, Msg::CopyHint))
            } else {
                String::new()
            };
            let expected = format!(
                "{prefix}  ↵/l/→:{label}{target}{copy}  Space:{}",
                tr(sim.app.lang, Msg::WkActionsTitle).trim()
            );
            let expected: Vec<_> = expected.split("  ").collect();
            let tokens = crate::ui::status::hint_tokens(&sim.app);
            assert_eq!(
                tokens
                    .iter()
                    .take(expected.len())
                    .map(String::as_str)
                    .collect::<Vec<_>>(),
                expected
            );
            let rows: Vec<_> = crate::ops::help::sections(&sim.app)
                .into_iter()
                .flat_map(|s| s.rows)
                .collect();
            assert_eq!(rows.iter().filter(|(k, label)| k == "/" && label == tr(sim.app.lang, Msg::GitFilterByName)).count(), 1);
            assert!(!rows.iter().any(|(k, _)| k == "V"));
            assert!(!sim.app.project_marks_enabled());
            keys(&mut sim, "V");
            assert!(sim.app.project_frame().unwrap().marked.is_empty());
            keys(&mut sim, &format!("/{query}"));
            assert!(sim.app.project_frame().unwrap().filtering);
            assert_eq!(sim.app.project_visible_rows().len(), 1);
            keys(&mut sim, "no-match");
            assert!(sim.app.project_visible_rows().is_empty());
            press(&mut sim, KeyCode::Esc);
            assert_eq!(sim.app.project_visible_rows().len(), 1);
            assert_eq!(sim.app.project_frame().unwrap().key.screen, name);
        }
        press(&mut sim, KeyCode::Enter);
        assert_eq!(sim.app.project_frame().unwrap().key.screen, "detail");
        let expected = format!(
            "j/k:{}  h/l:{}  s:{}  g/G:{}  q/Esc:{}  ?:{}  y:{}",
            tr(sim.app.lang, Msg::Scroll),
            tr(sim.app.lang, Msg::HintHscroll),
            tr(sim.app.lang, Msg::FtDiffLayout),
            tr(sim.app.lang, Msg::HintEnds),
            tr(sim.app.lang, Msg::ProjectBack),
            tr(sim.app.lang, Msg::HintHelp),
            tr(sim.app.lang, Msg::CopyHint)
        );
        assert_eq!(
            crate::ui::status::hint_tokens(&sim.app),
            expected.split("  ").collect::<Vec<_>>()
        );
    }
    let mut sim = open(&root, config("git", "en"), (160, 20));
    for name in ["changes", "log", "graph", "branches", "worktrees"] {
        screen(&mut sim, name);
        let tokens = crate::ui::status::hint_tokens(&sim.app);
        let entries: Vec<_> = tokens.iter().flat_map(|t| t.split("  ")).collect();
        assert_eq!(
            entries
                .iter()
                .filter(|t| t
                    .split_once(':')
                    .is_some_and(|(k, _)| k.contains("Enter") || k.contains('↵')))
                .count(),
            1
        );
        let mut used = std::collections::BTreeSet::new();
        for entry in entries {
            if let Some((keys, _)) = entry.split_once(':') {
                for key in if keys == "/" {
                    vec!["/"]
                } else {
                    keys.split('/').collect()
                } {
                    assert!(used.insert(key), "{name}: duplicate {key}: {tokens:?}");
                }
            }
        }
        if name == "changes" {
            assert_eq!(&tokens[..2], ["Tab/Shift-Tab:screens", "j/k:move"]);
            assert_eq!(
                tokens
                    .iter()
                    .filter(|t| ["s/S:", "u/U:", "x:", "c:"]
                        .iter()
                        .any(|k| t.starts_with(k)))
                    .map(String::as_str)
                    .collect::<Vec<_>>(),
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
fn vcs_port_nested_changes_paths_compare_every_body_cell() {
    if isolated("vcs_port_nested_changes_paths_compare_every_body_cell") {
        return;
    }
    use ratatui::{
        buffer::Buffer,
        layout::Rect,
        style::{Color, Modifier, Style},
        text::{Line, Span},
        widgets::{Paragraph, Widget},
    };
    let root = super::vcs_changes::git_repo("vcs_port_nested_cells", true);
    let nested = root.join("sub/deep");
    std::fs::create_dir_all(&nested).unwrap();
    for launch in [root.as_path(), nested.as_path()] {
        for path_style in ["relative", "home", "full"] {
            let mut cfg = config("git", "en");
            cfg.ui.path_style = path_style.into();
            cfg.plugins
                .entry("git".into())
                .or_default()
                .insert("path_style".into(), toml::Value::String(path_style.into()));
            let mut sim = open(launch, cfg, (200, 20));
            assert_eq!(sim.app.tab.projects.projects[0].root, root.as_path());
            assert_eq!(sim.app.tab.root, launch);
            let content = sim.app.project_content().unwrap();
            let ProjectContent::List(list) = content.as_ref() else {
                panic!("list required")
            };
            assert_eq!(list.rows.len(), 7);
            let entries = [
                (true, "M", Color::Yellow, "a-staged.txt"),
                (false, "M", Color::Yellow, "b-unstaged.txt"),
                (false, "D", Color::Red, "c-deleted.txt"),
                (true, "R", Color::Cyan, "d-renamed.txt"),
                (false, "U", Color::LightGreen, "e-untracked.txt"),
                (true, "A", Color::Green, "f-added.txt"),
                (false, "M", Color::Yellow, "日本語.txt"),
            ];
            let lines: Vec<_> = entries
                .into_iter()
                .enumerate()
                .map(|(i, (staged, marker, color, name))| {
                    let path = root.join(name);
                    assert_eq!(list.rows[i].value, path.to_string_lossy());
                    let display = if path_style == "relative" {
                        if launch == root.as_path() {
                            format!("{}/{name}", root.file_name().unwrap().to_str().unwrap())
                        } else {
                            format!("../../{name}")
                        }
                    } else if path_style == "home" {
                        let home = std::path::PathBuf::from(std::env::var_os("HOME").unwrap());
                        path.strip_prefix(home)
                            .map(|p| format!("~/{}", p.display()))
                            .unwrap_or_else(|_| path.display().to_string())
                    } else {
                        path.display().to_string()
                    };
                    let line = Line::from(vec![
                        Span::raw(if staged { "● " } else { "  " }),
                        Span::styled(format!("{marker} "), Style::new().fg(color)),
                        Span::raw(display),
                    ]);
                    if i == 0 {
                        line.style(Style::new().add_modifier(Modifier::REVERSED))
                    } else {
                        line
                    }
                })
                .collect();
            let area = Rect::new(0, 2, 198, 16);
            let mut expected = Buffer::empty(area);
            Paragraph::new(lines).render(area, &mut expected);
            let frame = sim.project_frame();
            let mut actual = Buffer::empty(area);
            for y in area.y..area.bottom() {
                for x in area.x..area.right() {
                    actual[(x, y)] = frame[(x, y)].clone();
                }
            }
            super::project_frames::compare_frames(&expected, &actual)
                .unwrap_or_else(|e| panic!("{path_style}: {e}"));
        }
    }
}

#[test]
fn vcs_port_watch_head_refs_linked_index_and_refresh_filtered_identity() {
    if isolated("vcs_port_watch_head_refs_linked_index_and_refresh_filtered_identity") {
        return;
    }
    let root = super::vcs_log::repo(3);
    let mut sim = open(&root, config("git", "en"), (100, 8));
    screen(&mut sim, "log");
    for (i, path) in [(3, ".git/HEAD"), (4, ".git/refs/heads/main")] {
        super::vcs_log::commit(&root, i);
        assert_eq!(
            sim.app.project_visible_rows().len(),
            i,
            "watching uses injected events"
        );
        sim.app
            .tab
            .projects
            .inject_event(Ok(notify::Event::new(notify::EventKind::Modify(
                notify::event::ModifyKind::Any,
            ))
            .add_path(root.join(path))));
        drain(&mut sim);
        assert_eq!(sim.app.project_visible_rows().len(), i + 1);
    }
    let root = super::vcs_log::repo(3);
    let mut sim = open(&root, config("git", "en"), (100, 8));
    screen(&mut sim, "log");
    keys(&mut sim, "/Summary 作者");
    press(&mut sim, KeyCode::Enter);
    let selected = sim.app.project_act_context().selected.unwrap().value;
    super::vcs_log::commit(&root, 3);
    assert_eq!(sim.app.project_visible_rows().len(), 3);
    keys(&mut sim, "r");
    assert_eq!(sim.app.project_visible_rows().len(), 4);
    assert_eq!(
        sim.app.project_act_context().selected.unwrap().value,
        selected
    );
    assert_eq!(sim.app.project_frame().unwrap().cursor, 1);
    screen(&mut sim, "changes");
    std::fs::write(root.join("added"), "new\n").unwrap();
    keys(&mut sim, "r");
    sim.see("added");

    let main = super::vcs_changes::git_repo("vcs_port_linked_index", false);
    let linked = unique_tmp("vcs_port_linked_index");
    crate::test_support::run_git(
        &main,
        &[
            "worktree",
            "add",
            "-q",
            "--detach",
            linked.to_str().unwrap(),
        ],
    );
    let root = linked.canonicalize().unwrap();
    let mut sim = open(&root, config("git", "en"), (180, 20));
    assert!(sim.app.tab.projects.uses_injected_events());
    std::fs::write(root.join("a-staged.txt"), "changed\n").unwrap();
    crate::test_support::run_git(&root, &["add", "a-staged.txt"]);
    let gitdir = crate::git::git_dir(&root).unwrap();
    sim.app
        .tab
        .projects
        .inject_event(Ok(notify::Event::new(notify::EventKind::Modify(
            notify::event::ModifyKind::Any,
        ))
        .add_path(gitdir.join("index"))));
    drain(&mut sim);
    sim.see("● M");
    press(&mut sim, KeyCode::Enter);
    assert_eq!(sim.app.tab.preview_path, Some(root.join("a-staged.txt")));
    assert!(sim.app.is_git_diff_preview());
}

#[test]
fn vcs_port_declared_marks_and_branch_copy_help_follow_the_real_list() {
    if isolated("vcs_port_declared_marks_and_branch_copy_help_follow_the_real_list") {
        return;
    }
    use konoma_plugin::{Failure, Input, Manifest, OpenCx, Plugin, Session};
    use std::sync::Arc;
    struct Marked;
    impl Plugin for Marked {
        fn manifest(&self) -> Manifest {
            let mut m = konoma_vcs::Git.manifest();
            m.project
                .as_mut()
                .unwrap()
                .screens
                .iter_mut()
                .find(|s| s.id == "changes")
                .unwrap()
                .marks = true;
            m
        }
        fn open(&self, input: Input, cx: &OpenCx) -> Result<Box<dyn Session>, Failure> {
            konoma_vcs::Git.open(input, cx)
        }
    }
    let root = super::vcs_changes::branch_write_fixture();
    let token =
        crate::plugins::test_plugins::register_replacement("vcs_port_marks", Arc::new(Marked));
    for lang in ["en", "ja"] {
        let mut cfg = config("git", lang);
        cfg.plugins
            .extend(crate::plugins::test_plugins::marker_config(&token).plugins);
        let mut sim = open(&root, cfg, (160, 20));
        for (name, marks) in [("changes", true), ("log", false), ("branches", false)] {
            screen(&mut sim, name);
            assert_eq!(sim.app.project_marks_enabled(), marks);
            assert!(matches!(
                sim.app.project_content().unwrap().as_ref(),
                ProjectContent::List(_)
            ));
            let sections = crate::ops::help::sections_for(&sim.app, Context::Canvas);
            let project = sections
                .iter()
                .find(|s| s.title == tr(sim.app.lang, Msg::ProjectTitle))
                .unwrap();
            assert_eq!(project.rows.iter().any(|(k, _)| k == "V"), marks);
            if marks {
                assert!(project
                    .rows
                    .contains(&("V".into(), tr(sim.app.lang, Msg::ProjectMark).into())));
            }
            assert!(project
                .rows
                .contains(&("/".into(), tr(sim.app.lang, Msg::GitFilterByName).into())));
        }
        for other in [false, true] {
            if other {
                keys(&mut sim, "j");
            }
            let name = sim.app.project_act_context().selected.unwrap().value;
            let sections = crate::ops::help::sections_for(&sim.app, Context::Canvas);
            let git: Vec<_> = sections
                .iter()
                .filter(|s| s.title == "Git")
                .flat_map(|s| &s.rows)
                .map(|(_, desc)| desc.as_str())
                .collect();
            assert_eq!(
                git,
                [
                    tr(sim.app.lang, Msg::GitCheckout),
                    tr(sim.app.lang, Msg::GitNewBranch),
                    tr(sim.app.lang, Msg::GitDelete)
                ]
            );
            let navigation = sections
                .iter()
                .find(|s| s.title == tr(sim.app.lang, Msg::ProjectTitle))
                .unwrap();
            assert!(navigation.rows.iter().any(|(k, _)| k == "/"));
            assert!(!navigation.rows.iter().any(|(k, _)| k == "V"));
            let tokens = crate::ops::footer::tokens(&sim.app, Context::Canvas);
            let operations: Vec<_> = tokens
                .iter()
                .filter(|t| t.starts_with("Enter:") || t.starts_with("c:") || t.starts_with("d:"))
                .cloned()
                .collect();
            let expected = if lang == "en" {
                ["c:new", "d:delete"]
            } else {
                ["c:新規", "d:削除"]
            };
            assert_eq!(operations, expected);
            let menu = crate::ops::menu::menu(
                &sim.app,
                Context::Canvas,
                LeaderId::Copy,
                crate::ops::menu::View::Open,
            )
            .unwrap();
            assert_eq!(menu.items.len(), 1);
            assert_eq!(menu.items[0].key.code, KeyCode::Char('n'));
            assert_eq!(menu.items[0].label, tr(sim.app.lang, Msg::WkBranchName));
            keys(&mut sim, "y");
            assert_eq!(sim.app.pending_leader, Some(LeaderId::Copy));
            use ratatui::widgets::Widget;
            let mut expected =
                ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, 160, 1));
            let spans = konoma_widgets::footer_line::which_key_spans(
                &menu.title,
                &[("n".into(), tr(sim.app.lang, Msg::WkBranchName).into())],
                160,
            );
            ratatui::widgets::Paragraph::new(ratatui::text::Line::from(spans))
                .render(expected.area, &mut expected);
            let frame = sim.project_frame();
            for x in 0..160 {
                assert_eq!(
                    frame[(x, 19)],
                    expected[(x, 0)],
                    "branch copy menu cell {x}"
                );
            }
            press(&mut sim, KeyCode::Esc);
            clear_test_clipboard();
            keys(&mut sim, "yn");
            assert_eq!(get_test_clipboard(), Some(name.clone()));
            assert_eq!(
                sim.app.flash,
                Some(konoma_host_core::copy::copied_notice(sim.app.lang, &name))
            );
        }
    }
}

#[test]
fn vcs_port_jj_watch_refreshes_log_for_bare_and_colocated_repositories() {
    if !crate::vcs::jj::available() {
        return;
    }
    if isolated("vcs_port_jj_watch_refreshes_log_for_bare_and_colocated_repositories") {
        return;
    }
    for colocated in [false, true] {
        let root = super::vcs_log::jj_repo(colocated);
        let mut sim = open(&root, config("jj", "en"), (80, 8));
        screen(&mut sim, "log");
        let content = sim.app.project_content().unwrap();
        let ProjectContent::List(before) = content.as_ref() else {
            panic!("list required")
        };
        let before: Vec<_> = before.rows.iter().map(|r| r.value.clone()).collect();
        super::vcs_log::jj(&root, &["new", "-m", "watch new"]);
        sim.app
            .tab
            .projects
            .inject_event(Ok(notify::Event::new(notify::EventKind::Modify(
                notify::event::ModifyKind::Any,
            ))
            .add_path(root.join(".jj/repo/op_heads"))));
        drain(&mut sim);
        let content = sim.app.project_content().unwrap();
        let ProjectContent::List(list) = content.as_ref() else {
            panic!("list required")
        };
        assert_ne!(
            list.rows
                .iter()
                .map(|r| r.value.clone())
                .collect::<Vec<_>>(),
            before
        );
        let latest = konoma_vcs_core::vcs::jj::log(&root, 1)[0].id.clone();
        assert!(list.rows.iter().any(|r| r.value == latest));
    }
}
