//! Legacy branch cells and real writes through the project host.

use super::*;
use crate::app::project_screens::ProjectContent;
use crate::test_support::{run_git, TmpDir};

fn fixture(name: &str) -> TmpDir {
    let dir = git_repo(name, false);
    run_git(&dir, &["branch", "-m", "main"]);
    run_git(&dir, &["branch", "alpha"]);
    run_git(&dir, &["branch", "日本語"]);
    dir
}

fn open(root: &std::path::Path, lang: &str, width: u16) -> (Sim, Sim) {
    let (mut old, mut new) = pair(root, "git", "vcs_branch_parity", lang, width, 20);
    old.key('b');
    new.key('b');
    drain(&mut new);
    assert!(old.app.is_git_branches());
    assert_eq!(new.app.project_frame().unwrap().key.screen, "branches");
    (old, new)
}

fn same(old: &mut Sim, new: &mut Sim, width: u16) {
    let diff = compare_body(old, new, width, 20);
    assert!(
        diff.is_empty(),
        "{}",
        diff.iter().take(10).cloned().collect::<Vec<_>>().join("\n")
    );
}

#[test]
fn e2e_vcs_branches_cells_current_selection_move_and_filter_at_two_widths() {
    let dir = fixture("vcs_branch_cells");
    for width in [80, 160] {
        for lang in ["en", "ja"] {
            let (mut old, mut new) = open(&dir, lang, width);
            same(&mut old, &mut new, width);
            old.key('j');
            new.key('j');
            drain(&mut new);
            same(&mut old, &mut new, width);
            for query in ["ALP", "日本", "missing", "*", "  "] {
                old.key('/');
                new.key('/');
                for key in query.chars() {
                    old.key(key);
                    new.key(key);
                }
                drain(&mut new);
                same(&mut old, &mut new, width);
                old.esc();
                new.esc();
                drain(&mut new);
            }
        }
    }
}

fn values(sim: &Sim) -> Vec<(String, String)> {
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

use crate::app::project_screens::{ProjectDialog, ProjectPrompt};
use crate::i18n::{tr, Msg};
use crate::keymap::{Action, Backend, Context, LeaderId, Screen};

fn current_branch(root: &std::path::Path) -> Option<String> {
    let repo = konoma_vcs_core::git2::Repository::open(root).unwrap();
    let name = repo.head().unwrap().shorthand().ok().map(str::to_string);
    name
}

struct BranchPair {
    old: Sim,
    new: Sim,
    old_root: TmpDir,
    new_root: TmpDir,
}

impl BranchPair {
    fn new(lang: &str, read_only: bool) -> Self {
        let old_root = fixture("vcs_branch_old");
        let new_root = fixture("vcs_branch_new");
        let mut cfg = plugin_config("git", "vcs_branch_writes", lang);
        cfg.plugins
            .entry("git".into())
            .or_default()
            .insert("read_only".into(), toml::Value::Boolean(read_only));
        let mut old_cfg = cfg.clone();
        old_cfg.plugins.clear();
        old_cfg
            .plugins
            .entry("git".into())
            .or_default()
            .insert("read_only".into(), toml::Value::Boolean(read_only));
        let mut old = Sim::with_config_sized(&old_root, old_cfg, 160, 20);
        old.app.open_git_view();
        old.key('b');
        let mut new = Sim::with_config_sized(&new_root, cfg, 160, 20);
        new.app.tab.projects.wait_for_result();
        new.app.poll_projects();
        new.key('p');
        drain(&mut new);
        new.key('b');
        drain(&mut new);
        Self {
            old,
            new,
            old_root,
            new_root,
        }
    }

    fn key(&mut self, key: char) {
        self.old.key(key);
        self.new.key(key);
        drain(&mut self.new);
    }
    fn enter(&mut self) {
        self.old.enter();
        self.new.enter();
        drain(&mut self.new);
    }
    fn select(&mut self, name: &str) {
        if !self.old.app.is_git_branches() {
            self.old.key('b');
        }
        self.key('g');
        let index = self
            .old
            .app
            .git_branch_view()
            .iter()
            .position(|b| b.name == name)
            .unwrap();
        for _ in 0..index {
            self.key('j');
        }
        assert_eq!(
            values(&self.new)[self.new.app.project_frame().unwrap().cursor].1,
            name
        );
    }
    fn flash(&self) {
        assert_eq!(self.new.app.flash, self.old.app.flash);
    }
    fn input(&self, expected: &str) {
        let Some(ProjectDialog::Act {
            prompts,
            stage,
            buffer,
            ..
        }) = &self.new.app.project_dialog
        else {
            panic!("input required")
        };
        let ProjectPrompt::Text(input) = &prompts[*stage] else {
            panic!("text required")
        };
        assert_eq!(
            input.label.get(self.new.app.plugin_lang()),
            tr(self.old.app.lang, Msg::NewBranch)
        );
        assert_eq!(buffer, expected);
    }
}

#[test]
fn e2e_vcs_branches_checkout_current_and_other_branch_reload_and_tree_status() {
    for lang in ["en", "ja"] {
        let mut p = BranchPair::new(lang, false);
        for name in ["main", "alpha", "日本語"] {
            p.select(name);
            p.new.app.clear_project_tree_dirty();
            p.old.enter();
            handle_key(
                &mut p.new.app,
                KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            )
            .unwrap();
            for _ in 0..32 {
                p.new.app.poll_projects();
                if !p.new.app.project_jobs_pending() {
                    break;
                }
                p.new.app.wait_project_job();
            }
            assert!(p.new.app.project_tree_dirty());
            drain(&mut p.new);
            p.flash();
            assert_eq!(current_branch(&p.new_root).as_deref(), Some(name));
            assert_eq!(p.new.app.project_frame().unwrap().key.screen, "branches");
            assert!(values(&p.new).contains(&("current".into(), name.into())));
            assert!(!p.new.app.project_tree_dirty());
            assert_eq!(p.new.app.git_branch(), Some(name));
        }
        p.select("main");
        p.key('l');
        p.flash();
        assert_eq!(current_branch(&p.new_root).as_deref(), Some("main"));
        // A real checkout conflict must preserve git's exact failure text and the screen.
        for root in [&p.old_root, &p.new_root] {
            run_git(root, &["switch", "alpha"]);
            std::fs::write(root.join("a-staged.txt"), "branch change\n").unwrap();
            run_git(root, &["add", "."]);
            run_git(root, &["commit", "-qm", "alpha change"]);
            run_git(root, &["switch", "main"]);
            std::fs::write(root.join("a-staged.txt"), "conflicting local change\n").unwrap();
        }
        p.select("alpha");
        p.enter();
        p.flash();
        assert!(p
            .new
            .app
            .flash
            .as_ref()
            .unwrap()
            .contains("would be overwritten"));
        assert_eq!(current_branch(&p.new_root).as_deref(), Some("main"));
    }
}

#[test]
fn e2e_vcs_branches_create_success_blank_and_failure_keeps_typed_input() {
    for lang in ["en", "ja"] {
        let mut p = BranchPair::new(lang, false);
        p.key('c');
        p.input("");
        p.key(' ');
        p.key(' ');
        p.enter();
        p.flash();
        assert_eq!(
            p.new.app.flash.as_deref(),
            Some(tr(p.new.app.lang, Msg::NameEmpty))
        );
        assert!(p.new.app.project_dialog.is_none());
        p.key('c');
        p.input("");
        for key in "  alpha  ".chars() {
            p.key(key);
        }
        p.enter();
        p.flash();
        p.input("  alpha  ");
        assert!(p.new.app.flash.as_ref().unwrap().contains("already exists"));
        p.old.esc();
        p.new.esc();
        p.key('c');
        for key in "  created  ".chars() {
            p.key(key);
        }
        p.enter();
        p.flash();
        assert_eq!(current_branch(&p.new_root).as_deref(), Some("created"));
        assert_eq!(p.new.app.project_frame().unwrap().key.screen, "branches");
        assert!(values(&p.new).contains(&("current".into(), "created".into())));
    }
}

fn dialog_cells(sim: &mut Sim) -> Vec<ratatui::buffer::Cell> {
    let buf = sim
        .term
        .draw(|f| crate::ui::dialog::render(f, &sim.app, f.area()))
        .unwrap()
        .buffer
        .clone();
    // The same 66-column popup is centered in both 160x20 terminals.
    (6..13)
        .flat_map(|y| (47..113).map(move |x| (x, y)))
        .map(|point| buf[point].clone())
        .collect()
}

#[test]
fn e2e_vcs_branches_delete_dialog_safe_force_cancel_and_current_guard() {
    for lang in ["en", "ja"] {
        let mut p = BranchPair::new(lang, false);
        p.key('d');
        p.flash();
        assert!(p.new.app.project_dialog.is_none());
        assert_eq!(
            p.new.app.flash.as_deref(),
            Some(tr(p.new.app.lang, Msg::CannotDeleteCurrentBranch))
        );
        p.select("alpha");
        p.key('d');
        let actual = dialog_cells(&mut p.new);
        let expected = dialog_cells(&mut p.old);
        let differences: Vec<_> = actual
            .iter()
            .zip(&expected)
            .enumerate()
            .filter(|(_, (a, b))| a != b)
            .take(5)
            .collect();
        assert!(differences.is_empty(), "{differences:?}");
        p.old.app.flash = None;
        p.new.app.flash = None;
        let line = |sim: &Sim| {
            crate::ui::status::footer_spans(&sim.app, 160)
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>()
        };
        assert_eq!(line(&p.new), line(&p.old));
        p.key('n');
        p.flash();
        assert!(p.new.app.project_dialog.is_none());
        assert!(crate::vcs::refs(&p.new_root)
            .iter()
            .any(|r| r.name == "alpha"));
        p.key('d');
        p.old.esc();
        p.new.esc();
        drain(&mut p.new);
        p.flash();
        p.key('d');
        p.key('y');
        p.flash();
        assert!(!crate::vcs::refs(&p.new_root)
            .iter()
            .any(|r| r.name == "alpha"));
        for root in [&p.old_root, &p.new_root] {
            run_git(root, &["switch", "日本語"]);
            std::fs::write(root.join("unmerged"), "only here\n").unwrap();
            run_git(root, &["add", "."]);
            run_git(root, &["commit", "-qm", "unmerged"]);
            run_git(root, &["switch", "main"]);
        }
        p.select("日本語");
        p.key('d');
        p.key('y');
        p.flash();
        assert!(p
            .new
            .app
            .flash
            .as_ref()
            .unwrap()
            .contains("not fully merged"));
        assert!(crate::vcs::refs(&p.new_root)
            .iter()
            .any(|r| r.name == "日本語"));
        p.key('d');
        p.key('!');
        p.flash();
        assert!(!crate::vcs::refs(&p.new_root)
            .iter()
            .any(|r| r.name == "日本語"));
    }
}

#[test]
fn e2e_vcs_branches_read_only_guard_precedes_all_prompts_and_writes() {
    for lang in ["en", "ja"] {
        let mut p = BranchPair::new(lang, true);
        p.select("alpha");
        for key in [KeyCode::Enter, KeyCode::Char('c'), KeyCode::Char('d')] {
            p.old.press(key, KeyModifiers::NONE);
            p.new.press(key, KeyModifiers::NONE);
            drain(&mut p.new);
            p.flash();
            assert!(p.new.app.project_dialog.is_none());
            assert_eq!(current_branch(&p.new_root).as_deref(), Some("main"));
            assert_eq!(crate::vcs::refs(&p.new_root).len(), 3);
            assert!(p.new.app.flash.is_some());
        }
    }
}

#[test]
fn e2e_vcs_branches_help_marks_and_filter_follow_each_project_list_declaration() {
    use konoma_plugin::{Failure, Input, Manifest, OpenCx, Plugin, Session};

    struct MarkedChanges;
    impl Plugin for MarkedChanges {
        fn manifest(&self) -> Manifest {
            let mut manifest = konoma_vcs::Git.manifest();
            // Exercise marking without changing the production VCS screen's declaration.
            manifest
                .project
                .as_mut()
                .unwrap()
                .screens
                .iter_mut()
                .find(|screen| screen.id == "changes")
                .unwrap()
                .marks = true;
            manifest
        }

        fn open(&self, input: Input, cx: &OpenCx) -> Result<Box<dyn Session>, Failure> {
            konoma_vcs::Git.open(input, cx)
        }
    }

    let dir = fixture("vcs_branch_help_marks_filter");
    let token = test_plugins::register("vcs_branch_help_marks_filter", Arc::new(MarkedChanges));
    for lang in ["en", "ja"] {
        let mut cfg = test_plugins::marker_config(&token);
        cfg.ui.lang = lang.into();
        let mut new = Sim::with_config_sized(&dir, cfg, 160, 20);
        new.app.tab.projects.wait_for_result();
        new.app.poll_projects();
        new.key('p');
        drain(&mut new);
        for (screen, marks) in [("changes", true), ("log", false), ("branches", false)] {
            assert_eq!(new.app.project_frame().unwrap().key.screen, screen);
            assert_eq!(new.app.project_marks_enabled(), marks);
            assert!(matches!(
                new.app.project_content().unwrap().as_ref(),
                ProjectContent::List(_)
            ));
            let sections = crate::ops::help::sections_for(&new.app, Context::Canvas);
            let project = sections
                .iter()
                .find(|section| section.title == tr(new.app.lang, Msg::ProjectTitle))
                .unwrap();
            assert_eq!(
                project.rows.iter().any(|(key, _)| key == "V"),
                marks,
                "{lang} {screen}"
            );
            if marks {
                assert!(project
                    .rows
                    .contains(&("V".into(), tr(new.app.lang, Msg::ProjectMark).into())));
            }
            assert!(
                project
                    .rows
                    .contains(&("/".into(), tr(new.app.lang, Msg::GitFilterByName).into())),
                "{lang} {screen}"
            );
            new.press(KeyCode::Tab, KeyModifiers::NONE);
            drain(&mut new);
        }
    }
}

#[test]
fn e2e_vcs_branches_copy_menu_clipboard_footer_and_help_match_legacy_acts() {
    use crate::ops::menu::{menu, View};
    use konoma_host_core::clipboard::{clear_test_clipboard, get_test_clipboard};
    let dir = fixture("vcs_branch_copy");
    for lang in ["en", "ja"] {
        let (mut old, mut new) = open(&dir, lang, 160);
        for other_row in [false, true] {
            if other_row {
                old.key('j');
                new.key('j');
                drain(&mut new);
            }
            let ctx = Context::Screen(Backend::Git, Screen::Branches);
            let actions = [
                Action::BranchCheckout,
                Action::BranchCreate,
                Action::BranchDelete,
            ];
            let expected: Vec<_> = crate::ops::help::rows_for(&old.app, ctx)
                .into_iter()
                .filter(|r| r.actions.iter().any(|a| actions.contains(a)))
                .map(|r| r.desc)
                .collect();
            let sections = crate::ops::help::sections_for(&new.app, Context::Canvas);
            let actual: Vec<_> = sections
                .iter()
                .filter(|s| s.title == "Git")
                .flat_map(|s| &s.rows)
                .map(|(_, desc)| desc.clone())
                .collect();
            assert_eq!(actual, expected);
            let navigation = sections
                .iter()
                .find(|s| s.title == tr(new.app.lang, Msg::ProjectTitle))
                .unwrap();
            assert!(navigation.rows.iter().any(|(keys, _)| keys == "/"));
            assert!(!navigation.rows.iter().any(|(keys, _)| keys == "V"));
            let plugin_tokens: Vec<_> = crate::ops::footer::tokens(&new.app, Context::Canvas)
                .into_iter()
                .filter(|t| t.starts_with("Enter:") || t.starts_with("c:") || t.starts_with("d:"))
                .collect();
            let old_tokens: Vec<_> = crate::ops::footer::tokens(&old.app, ctx)
                .into_iter()
                .filter(|t| t.starts_with("Enter:") || t.starts_with("c:") || t.starts_with("d:"))
                .collect();
            assert_eq!(plugin_tokens, old_tokens);
            let legacy = menu(&old.app, ctx, LeaderId::BranchCopy, View::Open).unwrap();
            let current = menu(&new.app, Context::Canvas, LeaderId::Copy, View::Open).unwrap();
            assert_eq!(
                legacy
                    .items
                    .iter()
                    .map(|i| (i.key, &i.label))
                    .collect::<Vec<_>>(),
                current
                    .items
                    .iter()
                    .map(|i| (i.key, &i.label))
                    .collect::<Vec<_>>()
            );
            old.app
                .keymaps
                .leaders
                .get_mut(&LeaderId::BranchCopy)
                .unwrap()
                .title = Msg::CopyHint;
            old.key('y');
            new.key('y');
            let cells = |app: &App| {
                use ratatui::widgets::Widget;
                let area = ratatui::layout::Rect::new(0, 0, 160, 1);
                let mut buffer = ratatui::buffer::Buffer::empty(area);
                ratatui::widgets::Paragraph::new(ratatui::text::Line::from(
                    crate::ui::status::footer_spans(app, 160),
                ))
                .render(area, &mut buffer);
                buffer
            };
            assert_eq!(cells(&new.app), cells(&old.app));
            old.esc();
            new.esc();
            clear_test_clipboard();
            old.key('y');
            old.key('n');
            let name = get_test_clipboard().unwrap();
            let flash = old.app.flash.clone();
            clear_test_clipboard();
            new.key('y');
            new.key('n');
            drain(&mut new);
            assert_eq!(get_test_clipboard(), Some(name));
            assert_eq!(new.app.flash, flash);
        }
    }
}

#[test]
fn e2e_vcs_branches_empty_allows_create_and_has_no_row_actions() {
    let dir = sandbox("vcs_branch_empty");
    crate::test_support::init_git_repo(&dir);
    let cfg = plugin_config("git", "vcs_branch_empty", "en");
    let mut new = Sim::with_config_sized(&dir, cfg, 80, 20);
    new.app.tab.projects.wait_for_result();
    new.app.poll_projects();
    new.key('p');
    drain(&mut new);
    new.key('b');
    drain(&mut new);
    assert_eq!(values(&new), [("empty".into(), "".into())]);
    new.enter();
    drain(&mut new);
    assert!(new.app.project_dialog.is_none());
    new.key('d');
    drain(&mut new);
    assert!(new.app.project_dialog.is_none());
    new.key('c');
    assert!(new.app.project_dialog.is_some());
    new.enter();
    drain(&mut new);
    assert_eq!(new.app.flash.as_deref(), Some("name is empty"));
    assert!(new.app.project_dialog.is_none());
}

#[test]
fn e2e_vcs_branches_jj_bookmark_cells_copy_and_no_write_hints() {
    if !crate::vcs::jj::available() {
        return;
    }
    let dir = sandbox("vcs_bookmarks").into_canonical().unwrap();
    assert!(crate::test_support::run_jj(
        &dir,
        &["git", "init", "--no-colocate", "."]
    ));
    assert!(crate::test_support::run_jj(
        &dir,
        &["describe", "-m", "seed"]
    ));
    for name in ["alpha", "日本語"] {
        assert!(crate::test_support::run_jj(
            &dir,
            &["bookmark", "create", name]
        ));
    }
    for lang in ["en", "ja"] {
        let (mut old, mut new) = pair(&dir, "jj", "vcs_bookmark_parity", lang, 160, 20);
        old.key('b');
        new.key('b');
        drain(&mut new);
        same(&mut old, &mut new, 160);
        assert!(values(&new).iter().all(|(kind, _)| kind == "branch"));
        let tokens = crate::ops::footer::tokens(&new.app, Context::Canvas);
        assert!(!tokens
            .iter()
            .any(|t| t.starts_with("c:") || t.starts_with("d:") || t.starts_with("Enter:")));
        let plugin_help: Vec<_> = crate::ops::help::sections_for(&new.app, Context::Canvas)
            .into_iter()
            .filter(|s| s.title == "jj")
            .flat_map(|s| s.rows)
            .collect();
        assert!(plugin_help.is_empty());
        let menu = crate::ops::menu::menu(
            &new.app,
            Context::Canvas,
            LeaderId::Copy,
            crate::ops::menu::View::Open,
        )
        .unwrap();
        assert_eq!(menu.items.len(), 1);
        use konoma_host_core::clipboard::{clear_test_clipboard, get_test_clipboard};
        clear_test_clipboard();
        new.key('y');
        new.key('n');
        drain(&mut new);
        assert_eq!(get_test_clipboard().as_deref(), Some("alpha"));
        for key in [KeyCode::Enter, KeyCode::Char('c'), KeyCode::Char('d')] {
            new.press(key, KeyModifiers::NONE);
            drain(&mut new);
            assert!(new.app.project_dialog.is_none());
        }
        assert_eq!(crate::vcs::refs(&dir).len(), 2);
    }
}
