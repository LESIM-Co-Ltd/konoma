//! The moves that are new in the key redesign (plugin stage 2 / B3c, design §10-3): `o` opens the
//! file in another app, `p` and `!` reach the project's screen and tool from the previews, `Tab`
//! walks the version-control screens, `/` filters the changes list, the log and the graph, `n` /
//! `N` step through the changes, `r` reads again everywhere, `e` works in a diff and on the
//! changes list, and `Ctrl-t` opens a row in a new tab. Driven through the real `handle_key`;
//! **no process is ever started**: the launcher is replaced by a recorder.

use super::*;
use crate::app::office_open::{attempts_for_verb, plan, Attempt, OfficeKind, OfficeOs, Opener};
use crate::keymap::{Action, Context};
use crate::ops::action_enabled;
use std::sync::{Arc, Mutex};

type Log = Arc<Mutex<Vec<Attempt>>>;

/// Replace the launcher with a recorder that says every attempt started fine.
fn record(s: &mut Sim) -> Log {
    let log: Log = Arc::new(Mutex::new(Vec::new()));
    let l = log.clone();
    s.app.set_office_runner(Arc::new(move |a: &Attempt| {
        l.lock().unwrap().push(a.clone());
        Ok(Some(0))
    }));
    log
}

#[cfg(feature = "git")]
/// English, and no syntax coloring: a cold grammar shows "loading…" in place of the text, which
/// would make these tests depend on which test warmed it first.
fn cfg_plain() -> Config {
    let mut cfg = cfg_en();
    cfg.ui.syntax_highlight = false;
    cfg
}

fn os_open() -> &'static str {
    match OfficeOs::current() {
        OfficeOs::MacOs => "open",
        OfficeOs::Other => "xdg-open",
    }
}

/// The footer tokens of `ctx` as the footer plan builds them (before fitting to a width).
fn tokens(s: &Sim, ctx: Context) -> Vec<String> {
    crate::ops::footer::tokens(&s.app, ctx)
}

fn has_token(s: &Sim, ctx: Context, token: &str) -> bool {
    tokens(s, ctx).iter().any(|t| t == token)
}

fn flash_is(s: &Sim, want: crate::i18n::Msg) -> bool {
    s.app
        .flash
        .as_deref()
        .is_some_and(|f| f.contains(crate::i18n::tr(s.app.lang, want)))
}

// ---------------------------------------------------------------------------------------------
// `o`
// ---------------------------------------------------------------------------------------------

/// `o` on a plain file hands it to the OS default through the launcher seam (a worker in the app,
/// inline here), names that in the flash, and starts nothing in the test.
#[test]
fn e2e_o_in_the_tree_opens_a_file_with_the_os_default() {
    let dir = sandbox("nk_o_tree");
    seed_files(&dir);
    let root = canon(&dir);
    let mut s = Sim::with_config(&root, cfg_en());
    let log = record(&mut s);
    s.select("notes.txt");
    assert!(action_enabled(&s.app, Context::Tree, Action::OpenExternal));
    assert!(has_token(&s, Context::Tree, "o:open in app"));
    s.key('o');
    assert_eq!(
        *log.lock().unwrap(),
        vec![Attempt {
            opener: Opener::SystemDefault,
            prog: os_open().to_string(),
            args: vec![root.join("notes.txt").to_string_lossy().into_owned()],
        }]
    );
    assert!(flash_is(&s, crate::i18n::Msg::OfficeOpenedDefault));
}

/// Without the recorder the test launcher refuses, so a test that forgets the seam fails loudly
/// instead of starting a real `open`.
#[test]
fn e2e_o_never_reaches_a_real_launcher_in_tests() {
    let dir = sandbox("nk_o_refused");
    seed_files(&dir);
    let mut s = Sim::with_config(&canon(&dir), cfg_en());
    s.select("notes.txt");
    s.key('o');
    let f = s.app.flash.clone().unwrap_or_default();
    assert!(f.contains("process launching is disabled in tests"), "{f}");
}

/// A directory is not opened: no hint, and the key says why.
#[test]
fn e2e_o_on_a_directory_is_not_offered() {
    let dir = sandbox("nk_o_dir");
    seed_files(&dir);
    let mut s = Sim::with_config(&canon(&dir), cfg_en());
    let log = record(&mut s);
    s.select("src");
    assert!(!action_enabled(&s.app, Context::Tree, Action::OpenExternal));
    assert!(!has_token(&s, Context::Tree, "o:open in app"));
    s.key('o');
    assert!(log.lock().unwrap().is_empty());
    assert!(flash_is(&s, crate::i18n::Msg::NotAFile));
}

/// An Office file takes the Office chain, the same one `e` walks (`office_open::plan`), and
/// `[editor] ext` — which is `e`'s — does not reach `o`.
#[test]
fn e2e_o_on_an_office_file_walks_the_office_chain_and_ignores_editor_ext() {
    let dir = sandbox("nk_o_office");
    std::fs::write(dir.join("book.xlsx"), b"x").unwrap();
    std::fs::write(dir.join("deck.pptx"), b"x").unwrap();
    std::fs::write(dir.join("doc.docx"), b"x").unwrap();
    let root = canon(&dir);
    let mut cfg = cfg_en();
    cfg.editor.ext.insert("xlsx".into(), "vim".into());
    let mut s = Sim::with_config(&root, cfg);
    let log = record(&mut s);
    for (name, kind) in [
        ("book.xlsx", OfficeKind::Excel),
        ("deck.pptx", OfficeKind::PowerPoint),
        ("doc.docx", OfficeKind::Word),
    ] {
        log.lock().unwrap().clear();
        s.select(name);
        s.key('o');
        let want = plan(OfficeOs::current(), kind, &root.join(name));
        assert_eq!(
            log.lock().unwrap().first(),
            want.first(),
            "{name}: the first app of the chain"
        );
    }
    // `e` on the same xlsx goes to the user's editor (explicit entry), not to an app.
    s.select("book.xlsx");
    log.lock().unwrap().clear();
    s.key('e');
    assert!(log.lock().unwrap().is_empty());
    assert_eq!(
        s.app.take_pending_edit().map(|(p, _)| p),
        Some(root.join("book.xlsx"))
    );
}

/// The Office plugins' declared `open` chain is exactly the chain `e` walks, on both platforms.
#[test]
fn the_office_plugins_open_verb_is_the_chain_e_walks() {
    // Real files: the decision passes the preview's gate, which a missing path does not.
    let dir = sandbox("nk_open_chain");
    for (name, kind) in [
        ("a.xlsx", OfficeKind::Excel),
        ("a.ods", OfficeKind::Excel),
        ("a.docx", OfficeKind::Word),
        ("a.odt", OfficeKind::Word),
        ("a.pptx", OfficeKind::PowerPoint),
        ("a.odp", OfficeKind::PowerPoint),
    ] {
        let path = dir.join(name);
        std::fs::write(&path, b"").unwrap();
        let verb = crate::plugins::open_verb_of(
            &[],
            crate::plugins::plugin_id_of(&[], &[], &path).as_deref(),
        );
        for os in [OfficeOs::MacOs, OfficeOs::Other] {
            assert_eq!(
                attempts_for_verb(os, &verb, Some(kind), &path),
                plan(os, kind, &path),
                "{name} on {os:?}"
            );
        }
    }
    // Everything else opens with the OS default alone.
    for name in ["a.txt", "a.rs", "a.md", "a.csv", "a.png", "Makefile"] {
        let path = dir.join(name);
        std::fs::write(&path, b"").unwrap();
        let verb = crate::plugins::open_verb_of(
            &[],
            crate::plugins::plugin_id_of(&[], &[], &path).as_deref(),
        );
        for os in [OfficeOs::MacOs, OfficeOs::Other] {
            let got = attempts_for_verb(os, &verb, None, &path);
            assert_eq!(got.len(), 1, "{name}");
            assert_eq!(got[0].opener, Opener::SystemDefault, "{name}");
        }
    }
}

/// The Office plugins' declared `edit` verb, read through the shared decision, is exactly the
/// chain `plan` builds, on both platforms and for every kind (`e` used to build it directly).
#[test]
fn the_office_plugins_edit_verb_resolves_to_the_chain_plan_builds() {
    let dir = sandbox("nk_edit_chain");
    use konoma_plugin::launch::{resolve_launch, Launch, LaunchPolicy};
    let policy = LaunchPolicy {
        editor_ext_explicit: false,
        office_apps: true,
        open_links: true,
    };
    for (name, kind) in [
        ("a.xlsx", OfficeKind::Excel),
        ("a.ods", OfficeKind::Excel),
        ("a.docx", OfficeKind::Word),
        ("a.odt", OfficeKind::Word),
        ("a.pptx", OfficeKind::PowerPoint),
        ("a.odp", OfficeKind::PowerPoint),
    ] {
        let path = dir.join(name);
        std::fs::write(&path, b"").unwrap();
        let decl = crate::plugins::plugin_id_of(&[], &[], &path)
            .and_then(|id| crate::plugins::declared_verb_of(&[], &id, konoma_plugin::Verb::Edit))
            .unwrap_or_else(|| panic!("{name}: no edit verb declared"));
        for os in [OfficeOs::MacOs, OfficeOs::Other] {
            assert_eq!(
                resolve_launch(konoma_plugin::Verb::Edit, os, Some(&decl), &path, &policy),
                Launch::Gui(plan(os, kind, &path)),
                "{name} on {os:?}"
            );
        }
    }
    // The same on the platform this test runs on, spelled out.
    let path = std::path::Path::new("/work/nk/a.xlsx");
    let first = plan(OfficeOs::current(), OfficeKind::Excel, path).remove(0);
    if cfg!(target_os = "macos") {
        assert_eq!(first.args, ["-b", "com.microsoft.Excel", "/work/nk/a.xlsx"]);
    } else {
        assert_eq!(first.prog, "libreoffice");
    }
}

/// `e` on an Office document walks the declared chain through the launcher seam; on a text file it
/// raises the editor request; `[editor] ext` and `[external] office_apps` keep their meaning.
#[test]
fn e2e_e_follows_the_declared_edit_verb_and_the_settings() {
    let dir = sandbox("nk_e_declared");
    seed_files(&dir);
    for n in ["book.xlsx", "doc.docx", "deck.pptx"] {
        std::fs::write(dir.join(n), b"x").unwrap();
    }
    let root = canon(&dir);

    let mut s = Sim::with_config(&root, cfg_en());
    let log = record(&mut s);
    for (name, kind) in [
        ("book.xlsx", OfficeKind::Excel),
        ("doc.docx", OfficeKind::Word),
        ("deck.pptx", OfficeKind::PowerPoint),
    ] {
        log.lock().unwrap().clear();
        s.select(name);
        assert!(has_token(&s, Context::Tree, "e:open"), "{name}");
        s.key('e');
        assert_eq!(
            log.lock().unwrap().first(),
            plan(OfficeOs::current(), kind, &root.join(name)).first(),
            "{name}"
        );
        assert!(s.app.take_pending_edit().is_none(), "{name}");
    }
    // A text file goes to the editor.
    log.lock().unwrap().clear();
    s.select("notes.txt");
    s.key('e');
    assert!(log.lock().unwrap().is_empty());
    assert_eq!(
        s.app.take_pending_edit().map(|(p, _)| p),
        Some(root.join("notes.txt"))
    );

    // office_apps = false: `e` explains and starts nothing; the footer does not offer it.
    let mut cfg = cfg_en();
    cfg.external.office_apps = false;
    let mut s = Sim::with_config(&root, cfg);
    let log = record(&mut s);
    s.select("book.xlsx");
    assert!(!has_token(&s, Context::Tree, "e:open"));
    s.key('e');
    assert!(log.lock().unwrap().is_empty());
    assert!(s.app.take_pending_edit().is_none());
    assert!(flash_is(&s, crate::i18n::Msg::OfficeAppsDisabled));

    // An explicit `[editor] ext` wins even with office_apps = false.
    let mut cfg = cfg_en();
    cfg.external.office_apps = false;
    cfg.editor.ext.insert("xlsx".into(), "vim".into());
    let mut s = Sim::with_config(&root, cfg);
    let log = record(&mut s);
    s.select("book.xlsx");
    s.key('e');
    assert!(log.lock().unwrap().is_empty());
    assert_eq!(
        s.app.take_pending_edit().map(|(p, _)| p),
        Some(root.join("book.xlsx"))
    );
}

/// `e` reads the `edit` verb of the plugin that shows the file: a rule that gives a Word file to
/// the code plugin makes `e` open the editor, and the hint says "edit".
#[test]
fn e2e_e_reads_the_verb_of_the_plugin_the_rules_pick() {
    let dir = sandbox("nk_e_rule");
    std::fs::write(dir.join("doc.docx"), b"x").unwrap();
    let root = canon(&dir);
    let mut cfg = cfg_en();
    cfg.preview.rules.push(crate::config::Rule {
        glob: Some("*.docx".into()),
        builtin: Some("code".into()),
        ..Default::default()
    });
    let mut s = Sim::with_config(&root, cfg);
    let log = record(&mut s);
    s.select("doc.docx");
    assert!(has_token(&s, Context::Tree, "e:edit"));
    s.key('e');
    assert!(log.lock().unwrap().is_empty());
    assert_eq!(
        s.app.take_pending_edit().map(|(p, _)| p),
        Some(root.join("doc.docx"))
    );
}

/// The two switches keep their meaning: `open_links` stops the OS default, `office_apps` stops
/// the Office chain; a stopped `o` is neither offered nor started and says which switch it is.
#[test]
fn e2e_o_follows_the_external_switches() {
    let dir = sandbox("nk_o_switches");
    seed_files(&dir);
    std::fs::write(dir.join("book.xlsx"), b"x").unwrap();
    let root = canon(&dir);

    let mut cfg = cfg_en();
    cfg.external.open_links = false;
    let mut s = Sim::with_config(&root, cfg);
    let log = record(&mut s);
    s.select("notes.txt");
    assert!(!action_enabled(&s.app, Context::Tree, Action::OpenExternal));
    assert!(!has_token(&s, Context::Tree, "o:open in app"));
    s.key('o');
    assert!(log.lock().unwrap().is_empty());
    assert!(flash_is(&s, crate::i18n::Msg::ExternalOpenLinksDisabled));
    // The Office chain has its own switch.
    s.select("book.xlsx");
    assert!(action_enabled(&s.app, Context::Tree, Action::OpenExternal));
    s.key('o');
    assert_eq!(log.lock().unwrap().len(), 1);

    let mut cfg = cfg_en();
    cfg.external.office_apps = false;
    let mut s = Sim::with_config(&root, cfg);
    let log = record(&mut s);
    s.select("book.xlsx");
    assert!(!action_enabled(&s.app, Context::Tree, Action::OpenExternal));
    s.key('o');
    assert!(log.lock().unwrap().is_empty());
    assert!(flash_is(&s, crate::i18n::Msg::OfficeAppsDisabled));
    s.select("notes.txt");
    s.key('o');
    assert_eq!(log.lock().unwrap().len(), 1);
}

/// `o` in every preview opens the file on show.
#[test]
fn e2e_o_in_the_previews_opens_the_shown_file() {
    let dir = sandbox("nk_o_previews");
    seed_files(&dir);
    let root = canon(&dir);
    for (name, ctx) in [
        ("notes.txt", Context::Text),
        ("readme.md", Context::Document),
        ("data.csv", Context::Table),
    ] {
        let mut s = Sim::with_config(&root, cfg_en());
        let log = record(&mut s);
        s.select(name);
        s.enter();
        assert_eq!(s.app.key_context(), Some(ctx), "{name}");
        assert!(has_token(&s, ctx, "o:open in app"), "{name}");
        s.key('o');
        let got = log.lock().unwrap().clone();
        assert_eq!(got.len(), 1, "{name}");
        assert_eq!(
            got[0].args,
            vec![root.join(name).to_string_lossy().into_owned()]
        );
    }
}

#[test]
fn e2e_o_on_an_image_preview_opens_the_picture() {
    let dir = sandbox("nk_o_image");
    write_solid_png(&dir.join("pic.png"), 8, 8, [10, 20, 30]);
    let root = canon(&dir);
    let mut s = Sim::with_config(&root, cfg_en()).with_picker();
    let log = record(&mut s);
    s.select("pic.png");
    s.enter();
    assert_eq!(s.app.key_context(), Some(Context::Image));
    s.key('o');
    assert_eq!(log.lock().unwrap().len(), 1);
}

// ---------------------------------------------------------------------------------------------
// `p` and `!`
// ---------------------------------------------------------------------------------------------

/// `p` opens the project's screen from the tree and from every kind of preview, `q` comes back to
/// the preview; `!` queues the project's tool from the same places.
#[cfg(feature = "git")]
#[test]
fn e2e_p_and_bang_work_from_the_tree_and_every_preview() {
    let dir = sandbox("nk_p_bang");
    seed_repo(&dir);
    std::fs::write(dir.join("notes.md"), "# T\n\ntext\n").unwrap();
    std::fs::write(dir.join("data.csv"), "a,b\n1,2\n").unwrap();
    write_solid_png(&dir.join("pic.png"), 8, 8, [1, 2, 3]);
    let root = canon(&dir);
    for (name, ctx) in [
        ("", Context::Tree),
        ("a.rs", Context::Text),
        ("notes.md", Context::Document),
        ("data.csv", Context::Table),
        ("pic.png", Context::Image),
        ("a.rs:diff", Context::Diff),
    ] {
        let mut s = Sim::with_config(&root, cfg_en()).with_picker();
        s.app.ensure_git_status_now();
        s.draw();
        match name {
            "" => {}
            "a.rs:diff" => {
                s.select("a.rs");
                s.key('d');
            }
            f => {
                s.select(f);
                s.enter();
            }
        }
        assert_eq!(s.app.key_context(), Some(ctx), "{name}");
        assert!(action_enabled(&s.app, ctx, Action::OpenGitView), "{name}");
        assert!(action_enabled(&s.app, ctx, Action::GitLaunchTool), "{name}");
        let mode = s.app.tab.mode;
        s.key('p');
        assert!(s.app.is_git_view(), "{name}: p opens the project's screen");
        s.key('q');
        assert!(!s.app.is_git_view(), "{name}");
        assert_eq!(
            s.app.tab.mode, mode,
            "{name}: q comes back to where p was pressed"
        );
        assert_eq!(s.app.key_context(), Some(ctx), "{name}");
        s.key('!');
        assert!(s.app.take_launch_git_tool(), "{name}: ! queues the tool");
    }
}

/// Outside a repository neither is offered and both say why; `[external] git_tool = false`
/// stops the tool alone.
#[test]
fn e2e_p_and_bang_are_not_offered_outside_a_repository() {
    let dir = sandbox("nk_p_outside");
    seed_files(&dir);
    let root = canon(&dir);
    let mut s = Sim::with_config(&root, cfg_en());
    s.app.ensure_git_status_now();
    s.draw();
    for (name, ctx) in [
        ("", Context::Tree),
        ("notes.txt", Context::Text),
        ("data.csv", Context::Table),
    ] {
        if !name.is_empty() {
            s.select(name);
            s.enter();
        }
        assert!(!action_enabled(&s.app, ctx, Action::OpenGitView), "{name}");
        #[cfg(feature = "git")]
        assert!(
            !action_enabled(&s.app, ctx, Action::GitLaunchTool),
            "{name}"
        );
        s.key('p');
        assert!(!s.app.is_git_view(), "{name}");
        assert!(flash_is(&s, crate::i18n::Msg::NotAGitRepo), "{name}");
        s.key('!');
        assert!(!s.app.take_launch_git_tool(), "{name}");
        #[cfg(feature = "git")]
        assert!(flash_is(&s, crate::i18n::Msg::NotAGitRepo), "{name}");
        if !name.is_empty() {
            s.key('q');
        }
    }
}

#[cfg(feature = "git")]
#[test]
fn e2e_bang_follows_the_git_tool_switch() {
    let dir = sandbox("nk_bang_switch");
    seed_repo(&dir);
    let mut cfg = cfg_en();
    cfg.external.git_tool = false;
    let mut s = Sim::with_config(&canon(&dir), cfg);
    s.app.ensure_git_status_now();
    s.draw();
    assert!(!action_enabled(
        &s.app,
        Context::Tree,
        Action::GitLaunchTool
    ));
    s.key('!');
    assert!(!s.app.take_launch_git_tool());
    assert!(flash_is(&s, crate::i18n::Msg::ExternalGitToolDisabled));
    // `p` is unaffected by that switch.
    assert!(action_enabled(&s.app, Context::Tree, Action::OpenGitView));
}

// ---------------------------------------------------------------------------------------------
// Tab walks the screens
// ---------------------------------------------------------------------------------------------

#[cfg(feature = "git")]
fn shows(s: &Sim) -> &'static str {
    if s.app.is_git_worktrees() {
        "worktrees"
    } else if s.app.is_git_branches() {
        "branches"
    } else if s.app.is_git_graph() {
        "graph"
    } else if s.app.is_git_log() {
        "log"
    } else if s.app.is_git_view() {
        "changes"
    } else {
        "none"
    }
}

#[cfg(feature = "git")]
#[test]
fn e2e_tab_walks_the_git_screens_both_ways() {
    let dir = sandbox("nk_tab_cycle");
    seed_repo_history(&dir);
    let mut s = Sim::with_config_sized(&canon(&dir), cfg_en(), 140, 50);
    s.key('p');
    assert_eq!(shows(&s), "changes");
    assert!(has_token(
        &s,
        s.app.key_context().unwrap(),
        "Tab/⇧Tab:screens"
    ));
    let forward = ["log", "graph", "branches", "worktrees", "changes", "log"];
    for want in forward {
        s.tab();
        assert_eq!(shows(&s), want);
    }
    let back = [
        "changes",
        "worktrees",
        "branches",
        "graph",
        "log",
        "changes",
    ];
    for want in back {
        s.press(KeyCode::BackTab, KeyModifiers::SHIFT);
        assert_eq!(shows(&s), want);
    }
    // The direct keys still go where they went.
    s.key('b');
    assert_eq!(shows(&s), "branches");
    s.key('q');
    s.key('w');
    assert_eq!(shows(&s), "worktrees");
    // Tab on a screen is listed in the help, worded for the screens that are in the cycle.
    let ctx = s.app.key_context().unwrap();
    let rows = crate::ops::help::rows_for(&s.app, ctx);
    let row = rows
        .iter()
        .find(|r| r.actions.contains(&Action::GitScreenNext))
        .expect("a help row for Tab");
    assert_eq!(row.keys_text, "Tab / ⇧Tab");
    assert_eq!(row.desc, "cycle changes/log/graph/branches/worktrees");
}

/// jj lists no worktrees, so its cycle goes changes, log, graph, bookmarks and round.
#[cfg(feature = "git")]
#[test]
fn e2e_tab_skips_the_worktrees_in_a_jj_repository() {
    let Some(dir) = jj_seed_repo("nk_tab_jj") else {
        return;
    };
    let mut s = Sim::with_config_sized(&canon(&dir), cfg_en(), 140, 50);
    s.key('p');
    assert_eq!(shows(&s), "changes");
    let mut seen = Vec::new();
    for _ in 0..4 {
        s.tab();
        seen.push(shows(&s));
    }
    assert!(!seen.contains(&"worktrees"), "{seen:?}");
    assert_eq!(*seen.last().unwrap(), "changes", "{seen:?}");
    let rows = crate::ops::help::rows_for(
        &s.app,
        Context::Screen(crate::keymap::Backend::Jj, crate::keymap::Screen::Changes),
    );
    let row = rows
        .iter()
        .find(|r| r.actions.contains(&Action::GitScreenNext))
        .expect("a help row for Tab");
    assert_eq!(row.desc, "cycle changes/log/graph/bookmarks");
}

// ---------------------------------------------------------------------------------------------
// `/` on the changes list, the log and the graph
// ---------------------------------------------------------------------------------------------

#[cfg(feature = "git")]
#[test]
fn e2e_slash_filters_the_changes_list_and_the_operations_follow_the_row() {
    let dir = sandbox("nk_filter_changes");
    seed_repo(&dir);
    let mut s = Sim::with_config(&canon(&dir), cfg_en());
    s.key('p');
    s.see("a.rs");
    s.see("new.txt");
    s.key('/');
    assert!(s.app.is_git_view() && s.app.filtering_list().is_some());
    assert_eq!(s.app.surface(), crate::keymap::Surface::ListFilter);
    s.keys("new");
    // The footer is the input line, the title carries the query and the count.
    s.see("/new");
    s.see("(1)");
    s.dont_see("a.rs");
    assert_eq!(
        s.app
            .git_view_selected()
            .map(|p| p.file_name().unwrap().to_owned()),
        Some("new.txt".into())
    );
    // Enter keeps the query and gives the keys back: `s` stages the row that is shown.
    s.enter();
    assert!(s.app.filtering_list().is_none());
    assert_eq!(
        s.app
            .list_filter_query(crate::app::list_filter::ListId::Changes),
        "new"
    );
    s.key('s');
    assert_eq!(staged_of(&s.app, "new.txt"), Some(true));
    assert_eq!(staged_of(&s.app, "a.rs"), Some(false));
    // Esc drops the query (everything is back); the next Esc leaves the screen.
    s.esc();
    s.see("a.rs");
    assert!(s.app.is_git_view());
    s.esc();
    assert!(!s.app.is_git_view());
}

#[cfg(feature = "git")]
#[test]
fn e2e_slash_filter_moves_over_the_shown_rows_only() {
    let dir = sandbox("nk_filter_move");
    seed_repo(&dir);
    std::fs::write(dir.join("c.rs"), "fn c() {}\n").unwrap();
    let mut s = Sim::with_config(&canon(&dir), cfg_en());
    s.key('p');
    s.key('/');
    s.keys(".rs");
    s.enter();
    // a.rs and c.rs match, new.txt does not.
    let name = |s: &Sim| {
        s.app
            .git_view_selected()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
    };
    assert_eq!(name(&s), Some("a.rs".into()));
    s.key('j');
    assert_eq!(name(&s), Some("c.rs".into()));
    s.key('j');
    assert_eq!(
        name(&s),
        Some("c.rs".into()),
        "j stops at the last shown row"
    );
    s.key('G');
    assert_eq!(name(&s), Some("c.rs".into()));
    s.key('g');
    assert_eq!(name(&s), Some("a.rs".into()));
    // A query that matches nothing shows nothing and the cursor stays put.
    s.key('/');
    s.keys("zzz");
    s.see("(0)");
    s.esc();
    s.see("new.txt");
}

#[cfg(feature = "git")]
#[test]
fn e2e_slash_filters_the_log_by_the_text_shown() {
    let dir = sandbox("nk_filter_log");
    seed_repo_history(&dir);
    let mut s = Sim::with_hub_screen_keys(&canon(&dir));
    s.key('p');
    s.key('l');
    assert!(s.app.is_git_log());
    s.see("alphacommit");
    s.see("betacommit");
    s.key('/');
    assert_eq!(s.app.surface(), crate::keymap::Surface::ListFilter);
    s.keys("beta");
    s.see("/beta");
    s.dont_see("alphacommit");
    s.dont_see("gammacommit");
    let want = s
        .app
        .git_log_entries()
        .iter()
        .find(|c| c.summary == "betacommit")
        .map(|c| c.id.clone())
        .expect("the beta commit");
    assert_eq!(s.app.git_log_selected_id(), Some(want.clone()));
    s.enter();
    // The row that is shown is the one that opens.
    s.enter();
    assert!(s.app.is_git_detail());
    s.see("betacommit");
    s.key('q');
    assert_eq!(s.app.git_log_selected_id(), Some(want));
    // Esc: the query first, the screen second.
    s.esc();
    s.see("alphacommit");
    assert!(s.app.is_git_log());
    s.esc();
    assert!(!s.app.is_git_log());
}

#[cfg(feature = "git")]
#[test]
fn e2e_slash_filters_the_graph_and_hides_the_connectors() {
    let dir = sandbox("nk_filter_graph");
    seed_repo_history(&dir);
    let mut s = Sim::with_hub_screen_keys(&canon(&dir));
    s.key('p');
    s.key('g');
    assert!(s.app.is_git_graph());
    s.key('/');
    s.keys("gamma");
    s.dont_see("alphacommit");
    s.dont_see("betacommit");
    s.see("gammacommit");
    let row = s.app.git_graph_selected_row().cloned().expect("a row");
    assert!(row.subject.contains("gamma"), "{}", row.subject);
    s.enter();
    s.esc(); // the query goes first
    s.see("alphacommit");
    assert!(s.app.is_git_graph());
}

/// The branch and worktree lists keep their filter, now through the same component: a paste
/// reaches the worktree list's query too.
#[cfg(feature = "git")]
#[test]
fn e2e_branch_and_worktree_filters_are_the_same_component() {
    let dir = sandbox("nk_filter_branch");
    seed_repo_history(&dir);
    let mut s = Sim::with_config(&canon(&dir), cfg_en());
    s.key('p');
    s.key('b');
    s.key('/');
    s.keys("feat");
    assert_eq!(s.app.git_branch_query(), "feat");
    assert_eq!(s.app.git_branch_view().len(), 1);
    s.esc();
    assert_eq!(s.app.git_branch_query(), "");
    s.esc();
    s.key('w');
    s.key('/');
    s.keys("zz");
    assert_eq!(s.app.git_worktree_query(), "zz");
    s.esc();
    assert!(s.app.is_git_worktrees());
}

// ---------------------------------------------------------------------------------------------
// `n` / `N`
// ---------------------------------------------------------------------------------------------

/// A repository with `many.rs`: 240 lines of which 12 blocks (lines 10, 30, 50, ...) are changed
/// after the commit, and `clean.txt`, which is not.
#[cfg(feature = "git")]
fn change_blocks_repo(dir: &std::path::Path) -> Vec<usize> {
    let run = |args: &[&str]| {
        let out = std::process::Command::new("git")
            .current_dir(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {out:?}");
    };
    run(&["init", "-q", "."]);
    run(&["config", "user.email", "t@t"]);
    run(&["config", "user.name", "t"]);
    let body = |changed: &[usize]| -> String {
        (1..=240)
            .map(|n| {
                if changed.contains(&n) {
                    format!("changed line {n}\n")
                } else {
                    format!("line {n}\n")
                }
            })
            .collect()
    };
    std::fs::write(dir.join("many.rs"), body(&[])).unwrap();
    std::fs::write(dir.join("clean.txt"), "one\ntwo\n").unwrap();
    run(&["add", "-A"]);
    run(&["commit", "-q", "-m", "init"]);
    let lines: Vec<usize> = (0..12).map(|k| 10 + 20 * k).collect();
    std::fs::write(dir.join("many.rs"), body(&lines)).unwrap();
    lines
}

#[cfg(feature = "git")]
#[test]
fn e2e_n_steps_through_the_change_blocks_of_a_text_preview() {
    let dir = sandbox("nk_n_text");
    let lines = change_blocks_repo(&dir);
    let mut s = Sim::with_config(&canon(&dir), cfg_plain());
    s.select("many.rs");
    s.enter();
    assert_eq!(s.app.key_context(), Some(Context::Text));
    // The gutter has been drawn: there are changes, and the footer says n/N steps through them.
    assert!(
        has_token(&s, Context::Text, "n/N:change"),
        "{:?}",
        tokens(&s, Context::Text)
    );
    let caret = |s: &Sim| s.app.caret_line_for_test() + 1;
    s.key('n');
    assert_eq!(caret(&s), lines[0]);
    s.key('n');
    assert_eq!(caret(&s), lines[1]);
    s.key('N');
    assert_eq!(caret(&s), lines[0]);
    // Wraps at both ends.
    s.key('N');
    assert_eq!(caret(&s), lines[11]);
    s.key('n');
    assert_eq!(caret(&s), lines[0]);
}

/// With a search active the same keys step through its matches, and the footer says so.
#[cfg(feature = "git")]
#[test]
fn e2e_n_steps_through_search_matches_while_a_search_is_active() {
    let dir = sandbox("nk_n_search");
    let lines = change_blocks_repo(&dir);
    let mut s = Sim::with_config(&canon(&dir), cfg_plain());
    s.select("many.rs");
    s.enter();
    s.key('/');
    s.keys("line 7");
    s.enter();
    assert!(s.app.preview_search_query().is_some());
    assert!(!has_token(&s, Context::Text, "n/N:change"));
    let before = s.app.caret_line_for_test();
    s.key('n');
    let after = s.app.caret_line_for_test();
    assert!(
        !lines.contains(&(after + 1)) || after == before,
        "n went to a change block, not a match: {after}"
    );
    // Esc clears the search; n is the change step again.
    s.esc();
    assert!(s.app.preview_search_query().is_none());
    s.key('n');
    assert!(lines.contains(&(s.app.caret_line_for_test() + 1)));
}

/// An unchanged file has no change to step to: no hint, and the key says so.
#[cfg(feature = "git")]
#[test]
fn e2e_n_has_no_hint_and_does_nothing_in_an_unchanged_file() {
    let dir = sandbox("nk_n_clean");
    change_blocks_repo(&dir);
    let mut s = Sim::with_config(&canon(&dir), cfg_en());
    s.select("clean.txt");
    s.enter();
    assert!(!has_token(&s, Context::Text, "n/N:change"));
    let caret = s.app.caret_line_for_test();
    s.key('n');
    assert_eq!(s.app.caret_line_for_test(), caret);
    assert!(flash_is(&s, crate::i18n::Msg::NoChangesInFile));
}

/// A decorated document steps through the blocks that carry a change mark.
#[cfg(feature = "git")]
#[test]
fn e2e_n_steps_through_the_changed_blocks_of_a_document() {
    let dir = sandbox("nk_n_doc");
    let run = |args: &[&str]| {
        let out = std::process::Command::new("git")
            .current_dir(&dir)
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {out:?}");
    };
    run(&["init", "-q", "."]);
    run(&["config", "user.email", "t@t"]);
    run(&["config", "user.name", "t"]);
    let body = |changed: &[usize]| -> String {
        (1..=60)
            .map(|n| {
                if changed.contains(&n) {
                    format!("changed paragraph {n}\n\n")
                } else {
                    format!("paragraph {n}\n\n")
                }
            })
            .collect()
    };
    std::fs::write(dir.join("long.md"), body(&[])).unwrap();
    run(&["add", "-A"]);
    run(&["commit", "-q", "-m", "init"]);
    std::fs::write(dir.join("long.md"), body(&[8, 45])).unwrap();
    let mut s = Sim::with_config_sized(&canon(&dir), cfg_en(), 90, 20);
    s.select("long.md");
    s.enter();
    s.draw();
    assert_eq!(s.app.key_context(), Some(Context::Document));
    assert!(
        has_token(&s, Context::Document, "n/N:change"),
        "{:?}",
        tokens(&s, Context::Document)
    );
    let scroll = |s: &Sim| s.app.tab.preview_scroll;
    s.key('n');
    let first = scroll(&s);
    assert!(first > 0, "the first changed block is below the top");
    s.draw();
    s.key('n');
    let second = scroll(&s);
    assert!(second > first, "{second} > {first}");
    s.draw();
    s.key('n');
    assert_eq!(scroll(&s), first, "wraps to the first block");
    s.draw();
    s.key('N');
    assert_eq!(scroll(&s), second);
}

#[cfg(feature = "git")]
fn diff_starts(s: &mut Sim) -> Vec<usize> {
    let lines = s.app.git_diff_lines();
    crate::preview::gitdiff::change_block_starts(&lines)
}

#[cfg(feature = "git")]
#[test]
fn e2e_n_steps_through_the_change_blocks_of_a_diff_and_e_opens_at_the_block() {
    let dir = sandbox("nk_n_diff");
    let lines = change_blocks_repo(&dir);
    let root = canon(&dir);
    let mut s = Sim::with_config(&root, cfg_en());
    s.select("many.rs");
    s.key('d');
    assert_eq!(s.app.key_context(), Some(Context::Diff));
    let starts = diff_starts(&mut s);
    assert_eq!(starts.len(), 12, "{starts:?}");
    assert!(has_token(&s, Context::Diff, "n/N:change"));
    // `e` before any step opens at the first block.
    s.key('e');
    assert_eq!(
        s.app.take_pending_edit(),
        Some((root.join("many.rs"), Some(lines[0])))
    );
    let scroll = |s: &Sim| s.app.tab.preview_scroll as usize;
    // The view cannot scroll a late block to the top: the last screenful is where it stops.
    let total = s.app.git_diff_lines().len();
    let at = |s: &Sim, k: usize| starts[k].min(total - s.app.tab.preview_viewport as usize);
    s.key('n');
    assert_eq!(scroll(&s), at(&s, 0));
    s.key('n');
    assert_eq!(scroll(&s), at(&s, 1));
    s.key('e');
    assert_eq!(
        s.app.take_pending_edit(),
        Some((root.join("many.rs"), Some(lines[1])))
    );
    s.key('N');
    assert_eq!(scroll(&s), at(&s, 0));
    s.key('N');
    assert_eq!(scroll(&s), at(&s, 11), "wraps to the last block");
    s.key('e');
    assert_eq!(
        s.app.take_pending_edit(),
        Some((root.join("many.rs"), Some(lines[11]))),
        "e names the block n / N landed on, even where the last screenful cannot show it at the top"
    );
    // One more step forward wraps round to the first block (the position alone, clamped at the
    // end, could not tell it was on the last).
    s.key('n');
    assert_eq!(scroll(&s), at(&s, 0));
    s.key('e');
    assert_eq!(
        s.app.take_pending_edit(),
        Some((root.join("many.rs"), Some(lines[0])))
    );
}

/// In the side-by-side layout the same keys land on the rows the blocks are drawn at.
#[cfg(feature = "git")]
#[test]
fn e2e_n_in_a_split_diff_lands_on_the_block_rows() {
    let dir = sandbox("nk_n_split");
    change_blocks_repo(&dir);
    let mut s = Sim::with_config(&canon(&dir), cfg_en());
    s.select("many.rs");
    s.key('d');
    s.key('s'); // unified -> split
    assert!(s.app.diff_is_split(88));
    let diff = s.app.git_diff_lines();
    let starts = crate::preview::gitdiff::change_block_starts(&diff);
    s.key('n');
    s.key('n');
    let want = crate::preview::gitdiff::scroll_row_of_line(&diff, starts[1], true);
    assert_eq!(s.app.tab.preview_scroll as usize, want);
}

// ---------------------------------------------------------------------------------------------
// `r`
// ---------------------------------------------------------------------------------------------

#[test]
fn e2e_r_reads_a_preview_again() {
    let dir = sandbox("nk_r_preview");
    seed_files(&dir);
    let mut s = Sim::with_config(&canon(&dir), cfg_en());
    s.select("notes.txt");
    s.enter();
    s.see("alpha");
    std::fs::write(dir.join("notes.txt"), "ZEBRA\nline two\n").unwrap();
    s.dont_see("ZEBRA");
    s.key('r');
    s.see("ZEBRA");
}

#[cfg(feature = "git")]
#[test]
fn e2e_r_reads_a_diff_again() {
    let dir = sandbox("nk_r_diff");
    seed_repo(&dir);
    let mut s = Sim::with_config(&canon(&dir), cfg_en());
    s.select("a.rs");
    s.key('d');
    s.dont_see("QUOKKA");
    std::fs::write(dir.join("a.rs"), "fn quokka() { QUOKKA }\n").unwrap();
    s.key('r');
    s.see("QUOKKA");
}

#[cfg(feature = "git")]
#[test]
fn e2e_r_reads_the_lists_again() {
    let dir = sandbox("nk_r_lists");
    seed_repo_history(&dir);
    let root = canon(&dir);
    let git = |args: &[&str]| {
        let out = std::process::Command::new("git")
            .current_dir(&root)
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {out:?}");
    };
    let mut s = Sim::with_hub_screen_keys(&root);
    s.key('p');
    s.key('l');
    assert_eq!(s.app.git_log_entries()[0].summary, "gammacommit");
    git(&["commit", "-q", "--allow-empty", "-m", "deltacommit"]);
    s.key('r');
    assert_eq!(s.app.git_log_entries()[0].summary, "deltacommit");
    s.key('q');
    s.key('b');
    assert!(!s
        .app
        .git_branch_view()
        .iter()
        .any(|b| b.name == "fresh-branch"));
    git(&["branch", "fresh-branch"]);
    s.key('r');
    assert!(s
        .app
        .git_branch_view()
        .iter()
        .any(|b| b.name == "fresh-branch"));
}

// ---------------------------------------------------------------------------------------------
// `e` and `Ctrl-t` on the changes list
// ---------------------------------------------------------------------------------------------

#[cfg(feature = "git")]
#[test]
fn e2e_e_o_and_ctrl_t_act_on_the_row_of_the_changes_list() {
    let dir = sandbox("nk_list_row");
    seed_repo(&dir);
    let root = canon(&dir);
    let mut s = Sim::with_config(&root, cfg_en());
    let log = record(&mut s);
    s.key('p');
    let sel = s.app.git_view_selected().unwrap();
    assert!(action_enabled(
        &s.app,
        s.app.key_context().unwrap(),
        Action::RequestEdit
    ));
    s.key('e');
    assert_eq!(s.app.take_pending_edit(), Some((sel.clone(), None)));
    s.key('o');
    assert_eq!(log.lock().unwrap().len(), 1);
    assert_eq!(
        log.lock().unwrap()[0].args,
        vec![sel.to_string_lossy().into_owned()]
    );
    // Ctrl-t opens the row in a new tab and leaves the list in this one.
    assert_eq!(s.app.tab_count(), 1);
    s.ctrl('t');
    assert_eq!(s.app.tab_count(), 2);
    assert_eq!(s.app.tab.mode, Mode::Preview);
    assert_eq!(s.app.tab.preview_path.as_deref(), Some(sel.as_path()));
    s.key('[');
    assert!(s.app.is_git_view(), "the first tab still shows the list");
}

/// A deleted file has nothing to edit or open: neither key is offered on its row.
#[cfg(feature = "git")]
#[test]
fn e2e_a_deleted_row_offers_neither_e_nor_o() {
    let dir = sandbox("nk_list_deleted");
    seed_repo(&dir);
    std::fs::remove_file(dir.join("b.rs")).unwrap();
    let mut s = Sim::with_config(&canon(&dir), cfg_en());
    let log = record(&mut s);
    s.key('p');
    while s
        .app
        .git_view_selected()
        .is_none_or(|p| !p.ends_with("b.rs"))
    {
        s.key('j');
    }
    let ctx = s.app.key_context().unwrap();
    assert!(!action_enabled(&s.app, ctx, Action::RequestEdit));
    assert!(!action_enabled(&s.app, ctx, Action::OpenExternal));
    assert!(!action_enabled(&s.app, ctx, Action::OpenInNewTab));
    s.key('e');
    assert!(s.app.take_pending_edit().is_none());
    s.key('o');
    assert!(log.lock().unwrap().is_empty());
}

// ---------------------------------------------------------------------------------------------
// The help
// ---------------------------------------------------------------------------------------------

#[test]
fn e2e_the_help_names_the_new_keys_where_they_act() {
    let dir = sandbox("nk_help");
    seed_files(&dir);
    let mut s = Sim::with_config_sized(&canon(&dir), cfg_en(), 120, 60);
    s.select("notes.txt");
    s.enter();
    s.key('?');
    s.see("open in another app");
    s.see("refresh");
}

// ---------------------------------------------------------------------------------------------
// `e` and `o` decide which plugin takes a file the same way (the full resolution)
// ---------------------------------------------------------------------------------------------

const SQLITE_MAGIC: &[u8] = b"SQLite format 3\0";

/// A plugin that takes files by their content type alone and declares both launch verbs.
struct Mimed;

impl konoma_plugin::prelude::Plugin for Mimed {
    fn manifest(&self) -> konoma_plugin::prelude::Manifest {
        use konoma_plugin::prelude::*;
        use konoma_plugin::{LaunchMode, Launcher, Verb, VerbDecl};
        Manifest::new("x-nk-mimed", "0.1.0")
            .bind(Bind::mime("application/vnd.sqlite3"))
            .shows(ViewKind::Text)
            .verb(
                VerbDecl::new(Verb::Edit)
                    .launcher(Launcher::Command(vec!["nk-edit".into(), "{path}".into()]))
                    .mode(LaunchMode::Gui),
            )
            .verb(
                VerbDecl::new(Verb::Open)
                    .launcher(Launcher::Command(vec!["nk-open".into(), "{path}".into()]))
                    .mode(LaunchMode::Gui),
            )
    }

    fn open(
        &self,
        _: konoma_plugin::prelude::Input,
        _: &konoma_plugin::prelude::OpenCx,
    ) -> Result<Box<dyn konoma_plugin::prelude::Session>, konoma_plugin::prelude::Failure> {
        Err(konoma_plugin::prelude::Failure::unsupported(
            "not opened here",
        ))
    }
}

fn mimed_sim(name: &str, files: &[(&str, &[u8])]) -> (Sim, Log, crate::test_support::TmpDir) {
    let dir = sandbox(name);
    for (f, bytes) in files {
        std::fs::write(dir.join(f), bytes).unwrap();
    }
    let marker = crate::plugins::test_plugins::register(name, Arc::new(Mimed));
    let mut cfg = crate::plugins::test_plugins::marker_config(&marker);
    cfg.ui.lang = "en".into();
    let mut s = Sim::with_config(&canon(&dir), cfg);
    let log = record(&mut s);
    (s, log, dir)
}

fn progs(log: &Log) -> Vec<String> {
    log.lock().unwrap().iter().map(|a| a.prog.clone()).collect()
}

/// A plugin bound by content type alone: `e` walks its `edit` chain and `o` its `open` chain,
/// and the footer words `e` after the chain (no file name or extension says so).
#[test]
fn e2e_e_and_o_use_the_same_plugin_when_it_is_bound_by_mime_alone() {
    let (mut s, log, _dir) = mimed_sim("nk_mime_both", &[("blob.dat", SQLITE_MAGIC)]);
    s.select("blob.dat");
    assert!(
        has_token(&s, Context::Tree, "e:open"),
        "{:?}",
        tokens(&s, Context::Tree)
    );
    s.key('e');
    s.key('o');
    assert_eq!(progs(&log), vec!["nk-edit", "nk-open"]);
    assert!(s.app.take_pending_edit().is_none());
}

/// The same bytes under a name no glob knows are a file nobody takes: `e` opens the editor and
/// the footer says "edit"; nothing is left over from the other file.
#[test]
fn e2e_a_file_no_plugin_takes_keeps_the_editor_and_the_edit_hint() {
    let (mut s, log, _dir) = mimed_sim("nk_mime_none", &[("plain.dat", b"just text\n")]);
    s.select("plain.dat");
    assert!(has_token(&s, Context::Tree, "e:edit"));
    s.key('e');
    assert!(log.lock().unwrap().is_empty());
    assert!(s.app.take_pending_edit().is_some());
}

/// A `[[preview.rules]]` assignment by content type decides the plugin for `e` as it does for
/// the preview and `o`.
#[test]
fn e2e_e_and_o_follow_a_rule_that_matches_by_content_type() {
    let dir = sandbox("nk_rule_mime");
    std::fs::write(dir.join("data.bin"), SQLITE_MAGIC).unwrap();
    let mut cfg = cfg_en();
    cfg.preview.rules.push(crate::config::Rule {
        mime: Some("application/vnd.sqlite3".into()),
        builtin: Some("spreadsheet".into()),
        ..Default::default()
    });
    let mut s = Sim::with_config(&canon(&dir), cfg);
    let log = record(&mut s);
    s.select("data.bin");
    assert!(
        has_token(&s, Context::Tree, "e:open"),
        "{:?}",
        tokens(&s, Context::Tree)
    );
    s.key('e');
    s.key('o');
    let l = log.lock().unwrap();
    let want = |a: &Attempt| a.args.iter().any(|x| x.ends_with("data.bin"));
    assert!(l.len() >= 2, "both keys started their chain: {l:?}");
    assert!(l.iter().all(want), "{l:?}");
    assert!(s.app.take_pending_edit().is_none());
}

/// The answer is remembered between frames but not past a change of the file: the bytes decide
/// the plugin, so a rewrite (even to the same size) is decided again.
#[test]
fn e2e_e_decides_again_when_the_file_changes() {
    let (mut s, log, dir) = mimed_sim("nk_mime_change", &[("flip.dat", SQLITE_MAGIC)]);
    let p = dir.join("flip.dat");
    let mut same = SQLITE_MAGIC.to_vec();
    s.select("flip.dat");
    assert!(has_token(&s, Context::Tree, "e:open"));
    assert!(has_token(&s, Context::Tree, "e:open"), "second frame, memo");
    // Same size, other bytes, later mtime.
    same.fill(b'x');
    std::fs::write(&p, &same).unwrap();
    let later = std::time::SystemTime::now() + std::time::Duration::from_secs(10);
    std::fs::File::options()
        .write(true)
        .open(&p)
        .unwrap()
        .set_modified(later)
        .unwrap();
    assert!(
        has_token(&s, Context::Tree, "e:edit"),
        "{:?}",
        tokens(&s, Context::Tree)
    );
    s.key('e');
    assert!(log.lock().unwrap().is_empty());
    assert!(s.app.take_pending_edit().is_some());
}
