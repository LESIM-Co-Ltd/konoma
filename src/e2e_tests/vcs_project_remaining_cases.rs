// Independent literal recipes for the old tests outside the legacy comparison modules.

fn selected_value(sim: &Sim) -> String {
    sim.app.project_act_context().selected.unwrap().value
}

fn shown_values(sim: &Sim) -> Vec<String> {
    let content = sim.app.project_content().unwrap();
    let ProjectContent::List(list) = content.as_ref() else {
        panic!("list required")
    };
    sim.app
        .project_visible_rows()
        .into_iter()
        .map(|i| list.rows[i].value.clone())
        .collect()
}

#[test]
fn vcs_port_original_log_detail_history_and_six_copy_keys() {
    if isolated("vcs_port_original_log_detail_history_and_six_copy_keys") {
        return;
    }
    let root = sandbox("port_original_log_history");
    seed_repo_history(&root);
    let mut sim = open(&root, config("git", "en"), (180, 30));
    assert_eq!(sim.app.project_frame().unwrap().key.screen, "changes");
    screen(&mut sim, "log");
    for text in ["alphacommit", "betacommit", "gammacommit"] {
        sim.see(text);
    }
    let top = selected_value(&sim);
    keys(&mut sim, "j");
    let second = selected_value(&sim);
    assert_ne!(second, top, "log j selects the next commit");
    let subject = crate::git::commit_meta(&root, &second)
        .unwrap()
        .message
        .lines()
        .next()
        .unwrap()
        .to_string();
    for code in [KeyCode::Enter, KeyCode::Char('l')] {
        press(&mut sim, code);
        assert_eq!(sim.app.project_frame().unwrap().key.screen, "detail");
        sim.see(&subject);
        keys(&mut sim, "q");
        assert_eq!(sim.app.project_frame().unwrap().key.screen, "log");
        assert_eq!(selected_value(&sim), second);
    }
    screen(&mut sim, "graph");
    keys(&mut sim, "g");
    for detail in [false, true] {
        if detail {
            press(&mut sim, KeyCode::Enter);
        }
        assert_eq!(
            sim.app.project_frame().unwrap().key.screen,
            if detail { "detail" } else { "graph" }
        );
        // Old hub: Context::Screen(backend, Graph) / Screen(backend, Detail).
        assert_eq!(
            sim.app.canvas_screen(),
            Some(crate::keymap::Context::Screen(
                crate::keymap::Backend::Git,
                if detail {
                    crate::keymap::Screen::Detail
                } else {
                    crate::keymap::Screen::Graph
                }
            )),
            "graph/detail key context"
        );
        keys(&mut sim, "y");
        assert_eq!(sim.app.pending_leader, Some(LeaderId::Copy));
        press(&mut sim, KeyCode::Esc);
        assert_eq!(sim.app.pending_leader, None);
    }

    let root = sandbox("port_original_copy_values");
    seed_repo(&root);
    let mut sim = open(&root, config("git", "en"), (180, 30));
    screen(&mut sim, "log");
    sim.see("init");
    // Old hub: Context::Screen(backend, Log).
    assert_eq!(
        sim.app.canvas_screen(),
        Some(crate::keymap::Context::Screen(
            crate::keymap::Backend::Git,
            crate::keymap::Screen::Log
        ))
    );
    assert_eq!(sim.app.key_context(), Some(crate::keymap::Context::Canvas));
    let meta = crate::git::commit_meta(&root, &selected_value(&sim)).unwrap();
    assert_eq!(meta.message.lines().next(), Some("init"));
    assert_eq!(meta.author, "t");
    assert_eq!(meta.short.len(), 7);
    assert_eq!(meta.id.len(), 40);
    assert!(meta.id.starts_with(&meta.short));
    assert!(!meta.date.is_empty());
    for (key, expected) in [
        ('s', meta.short.clone()),
        ('h', meta.id.clone()),
        ('t', "init".into()),
        ('m', meta.message.clone()),
        ('a', "t".into()),
        ('d', meta.date.clone()),
    ] {
        clear_test_clipboard();
        keys(&mut sim, "y");
        assert_eq!(
            sim.app.pending_leader,
            Some(LeaderId::Copy),
            "copy leader {key}"
        );
        press(&mut sim, KeyCode::Char(key));
        assert_eq!(sim.app.pending_leader, None, "copy consumed {key}");
        assert_eq!(get_test_clipboard(), Some(expected), "copy value {key}");
    }
    press(&mut sim, KeyCode::Enter);
    assert_eq!(sim.app.project_frame().unwrap().key.screen, "detail");
    sim.see("First body paragraph.");
    sim.see("Second body paragraph.");
    keys(&mut sim, "q");
    assert_eq!(sim.app.project_frame().unwrap().key.screen, "log");
}

#[test]
fn vcs_port_original_graph_selection_base_picker_and_config() {
    if isolated("vcs_port_original_graph_selection_base_picker_and_config") {
        return;
    }
    let root = sandbox("port_original_graph_history");
    seed_repo_history(&root);
    let mut sim = open(&root, config("git", "en"), (180, 30));
    screen(&mut sim, "graph");
    sim.see("●");
    assert_eq!(sim.app.project_frame().unwrap().key.screen, "graph");
    keys(&mut sim, "g");
    let top = selected_value(&sim);
    keys(&mut sim, "G");
    let bottom = selected_value(&sim);
    assert_ne!(top, bottom);
    keys(&mut sim, "gj");
    assert_ne!(selected_value(&sim), top);
    assert_eq!(
        sim.app.project_act_context().selected.unwrap().kind,
        "commit"
    );
    sim.dont_see("⌖ base:");
    // A row below the top, so a selection that fell back to the first row would be noticed.
    keys(&mut sim, "gj");
    let selected = selected_value(&sim);
    assert_ne!(selected, top, "the base pin is tried on a row that is not the first");
    keys(&mut sim, "s");
    sim.see("⌖ base:");
    assert_eq!(selected_value(&sim), selected);
    keys(&mut sim, "0");
    sim.see("⌖ base:");
    keys(&mut sim, "x");
    sim.dont_see("⌖ base:");
    assert_eq!(selected_value(&sim), selected, "x keeps the selected id");
    keys(&mut sim, "b");
    assert_eq!(sim.app.project_frame().unwrap().key.screen, "graph_picker");
    sim.see("feature-x");
    let mark = |sim: &Sim| {
        let content = sim.app.project_content().unwrap();
        let ProjectContent::List(list) = content.as_ref() else {
            panic!("list required")
        };
        list.rows
            .iter()
            .find(|r| r.value == "feature-x")
            .unwrap()
            .cells[0]
            .plain()
    };
    let on = "[x]   feature-x";
    let off = "[ ]   feature-x";
    assert_eq!(mark(&sim), on);
    keys(&mut sim, "n");
    assert_eq!(mark(&sim), on, "picker n is inert");
    keys(&mut sim, "c");
    assert_eq!(mark(&sim), off);
    keys(&mut sim, "a");
    assert_eq!(mark(&sim), on);
    keys(&mut sim, "q");
    assert!(!sim.app.project_pushed());
    assert_eq!(sim.app.project_frame().unwrap().key.screen, "graph");
    keys(&mut sim, "bcl");
    assert_eq!(sim.app.project_frame().unwrap().key.screen, "graph");
    assert!(!sim.app.project_pushed());

    for name in ["b1", "b2", "b3"] {
        crate::test_support::run_git(&root, &["branch", name]);
    }
    for cap in [2, 0] {
        let mut cfg = config("git", "en");
        cfg.plugins
            .entry("git".into())
            .or_default()
            .insert("graph_max_branches".into(), toml::Value::Integer(cap));
        let mut sim = open(&root, cfg, (180, 30));
        screen(&mut sim, "graph");
        if cap == 0 {
            sim.dont_see("hidden)");
        } else {
            sim.see("hidden)");
        }
    }
    let base = crate::git::branch(&root).unwrap();
    for auto in [false, true] {
        let mut cfg = config("git", "en");
        if auto {
            cfg.plugins.entry("git".into()).or_default().insert(
                "graph_base_branches".into(),
                toml::Value::Array(vec![toml::Value::String(base.clone())]),
            );
        }
        let mut sim = open(&root, cfg, (180, 30));
        screen(&mut sim, "graph");
        if auto {
            sim.see("⌖ base:");
        } else {
            sim.dont_see("⌖ base:");
        }
    }
}

#[test]
fn vcs_port_original_changes_filters_row_actions_and_refresh() {
    if isolated("vcs_port_original_changes_filters_row_actions_and_refresh") {
        return;
    }
    let root = sandbox("port_original_changes_filter");
    seed_repo(&root);
    std::fs::write(root.join("c.rs"), "fn c() {}\n").unwrap();
    let mut sim = open(&root, config("git", "en"), (180, 30));
    sim.see("a.rs");
    sim.see("new.txt");
    keys(&mut sim, "/new");
    assert!(sim.app.project_frame().unwrap().filtering);
    sim.see("/new");
    sim.dont_see("a.rs");
    assert_eq!(
        sim.app.project_row_target(),
        Some(canon(&root).join("new.txt"))
    );
    press(&mut sim, KeyCode::Enter);
    assert!(!sim.app.project_frame().unwrap().filtering);
    assert_eq!(sim.app.project_frame().unwrap().query, "new");
    keys(&mut sim, "s");
    let status = crate::git::changed_files(&root);
    assert!(
        status
            .iter()
            .find(|f| f.path.ends_with("new.txt"))
            .unwrap()
            .staged
    );
    assert!(
        !status
            .iter()
            .find(|f| f.path.ends_with("a.rs"))
            .unwrap()
            .staged
    );
    press(&mut sim, KeyCode::Esc);
    sim.see("a.rs");
    assert_eq!(sim.app.project_frame().unwrap().key.screen, "changes");
    press(&mut sim, KeyCode::Esc);
    assert!(!sim.app.project_active());
    let mut sim = open(&root, config("git", "en"), (180, 30));
    keys(&mut sim, "/.rs");
    press(&mut sim, KeyCode::Enter);
    for (key, name) in [
        (None, "a.rs"),
        (Some('j'), "c.rs"),
        (Some('j'), "c.rs"),
        (Some('G'), "c.rs"),
        (Some('g'), "a.rs"),
    ] {
        if let Some(c) = key {
            press(&mut sim, KeyCode::Char(c));
        }
        assert_eq!(
            sim.app.project_row_target(),
            Some(canon(&root).join(name)),
            "filtered cursor {key:?}"
        );
    }
    keys(&mut sim, "/zzz");
    assert!(shown_values(&sim).is_empty());
    press(&mut sim, KeyCode::Esc);
    sim.see("new.txt");
    keys(&mut sim, "G");
    let last = selected_value(&sim);
    keys(&mut sim, "g");
    assert_ne!(selected_value(&sim), last);
    assert_eq!(sim.app.project_frame().unwrap().key.screen, "changes");
    press(&mut sim, KeyCode::Esc);
    assert!(!sim.app.project_active());

    let root = sandbox("port_original_refresh");
    seed_repo_history(&root);
    let mut sim = open(&root, config("git", "en"), (180, 30));
    screen(&mut sim, "log");
    assert!(sim.app.project_content().is_some());
    sim.see("gammacommit");
    crate::test_support::run_git(
        &root,
        &["commit", "-q", "--allow-empty", "-m", "deltacommit"],
    );
    keys(&mut sim, "r");
    sim.see("deltacommit");
    screen(&mut sim, "branches");
    assert!(!shown_values(&sim).contains(&"fresh-branch".to_owned()));
    crate::test_support::run_git(&root, &["branch", "fresh-branch"]);
    keys(&mut sim, "r");
    assert!(shown_values(&sim).contains(&"fresh-branch".to_owned()));
}

#[test]
fn vcs_port_original_log_and_graph_filter_identity() {
    if isolated("vcs_port_original_log_and_graph_filter_identity") {
        return;
    }
    let root = sandbox("port_original_history_filter");
    seed_repo_history(&root);
    let mut sim = open(&root, config("git", "en"), (180, 30));
    screen(&mut sim, "log");
    sim.see("alphacommit");
    sim.see("betacommit");
    keys(&mut sim, "/beta");
    assert!(sim.app.project_frame().unwrap().filtering);
    sim.see("/beta");
    sim.dont_see("alphacommit");
    sim.dont_see("gammacommit");
    let want = crate::git::log(&root, 200)
        .into_iter()
        .find(|c| c.summary == "betacommit")
        .unwrap()
        .id;
    assert_eq!(selected_value(&sim), want);
    press(&mut sim, KeyCode::Enter);
    press(&mut sim, KeyCode::Enter);
    assert_eq!(sim.app.project_frame().unwrap().key.screen, "detail");
    sim.see("betacommit");
    keys(&mut sim, "q");
    assert_eq!(selected_value(&sim), want);
    press(&mut sim, KeyCode::Esc);
    sim.see("alphacommit");
    assert_eq!(sim.app.project_frame().unwrap().key.screen, "log");
    press(&mut sim, KeyCode::Esc);
    assert!(!sim.app.project_active());
    let mut sim = open(&root, config("git", "en"), (180, 30));
    screen(&mut sim, "graph");
    keys(&mut sim, "/gamma");
    sim.dont_see("alphacommit");
    sim.dont_see("betacommit");
    sim.see("gammacommit");
    let selected = selected_value(&sim);
    assert!(crate::git::commit_meta(&root, &selected)
        .unwrap()
        .message
        .contains("gamma"));
    assert!(
        shown_values(&sim).iter().all(|value| !value.is_empty()),
        "filtered graph has no connectors"
    );
    press(&mut sim, KeyCode::Enter);
    press(&mut sim, KeyCode::Esc);
    sim.see("alphacommit");
    assert_eq!(sim.app.project_frame().unwrap().key.screen, "graph");
    keys(&mut sim, "q");
    assert!(!sim.app.project_active());
}

#[test]
fn vcs_port_original_git_and_jj_cycles_and_contexts() {
    if isolated("vcs_port_original_git_and_jj_cycles_and_contexts") {
        return;
    }
    let git = sandbox("port_original_cycle");
    seed_repo(&git);
    let cycle = |root: &std::path::Path, id: &str, forward: Vec<&str>, back: Vec<&str>| {
        let mut sim = open(root, config(id, "en"), (180, 30));
        assert_eq!(sim.app.project_frame().unwrap().key.screen, "changes");
        for (code, expected) in forward
            .into_iter()
            .map(|s| (KeyCode::Tab, s))
            .chain(back.into_iter().map(|s| (KeyCode::BackTab, s)))
        {
            press(&mut sim, code);
            assert_eq!(sim.app.project_frame().unwrap().key.screen, expected);
            // Old hub: Context::Screen(backend, screen) after every Tab / BackTab.
            {
                use crate::keymap::{Backend, Context, Screen};
                let backend = if id == "git" { Backend::Git } else { Backend::Jj };
                let screen = match expected {
                    "changes" => Screen::Changes,
                    "log" => Screen::Log,
                    "graph" => Screen::Graph,
                    "branches" => Screen::Branches,
                    "worktrees" => Screen::Worktrees,
                    other => panic!("unexpected screen {other}"),
                };
                assert_eq!(
                    sim.app.canvas_screen(),
                    Some(Context::Screen(backend, screen)),
                    "{id}/{expected} after Tab/BackTab"
                );
            }
            assert_eq!(
                sim.app
                    .tab
                    .project_navigation
                    .as_ref()
                    .unwrap()
                    .project
                    .plugin,
                id
            );
        }
        keys(&mut sim, "b");
        assert_eq!(sim.app.project_frame().unwrap().key.screen, "branches");
        if id == "git" {
            keys(&mut sim, "w");
            assert_eq!(sim.app.project_frame().unwrap().key.screen, "worktrees");
        }
        let rows = crate::ops::help::sections(&sim.app)
            .into_iter()
            .flat_map(|s| s.rows)
            .collect::<Vec<_>>();
        // The help row of the screen cycle must exist; its key label and description are the
        // pending `tab-help` decision below, so only its presence is asserted.
        assert!(
            rows.iter().any(|(keys, _)| keys.contains("Tab")),
            "help has a Tab row for the screen cycle on {id}: {rows:?}"
        );
        super::vcs_port_map::pending_behavior("tab-help", "Tab / ⇧Tab; cycle changes/log/graph/branches/worktrees; cycle changes/log/graph/bookmarks");
        screen(&mut sim, "log");
        press(&mut sim, KeyCode::Enter);
        assert_eq!(sim.app.project_frame().unwrap().key.screen, "detail");
    };
    cycle(
        &git,
        "git",
        vec!["log", "graph", "branches", "worktrees", "changes", "log"],
        vec![
            "changes",
            "worktrees",
            "branches",
            "graph",
            "log",
            "changes",
        ],
    );
    // Only the jj half needs the jj binary; the Git half above has already run.
    if !crate::vcs::jj::available() {
        return;
    }
    let jj = super::vcs_log::jj_repo(false);
    cycle(
        &jj,
        "jj",
        vec!["log", "graph", "branches", "changes"],
        vec!["branches", "graph", "log", "changes"],
    );
}
