// Launch eligibility is separate from the plugin's row data target.
fn launch_host(root: &Path, text: &str) -> Sim {
    let mut host = Sim::with_config_sized(root, toml::from_str(text).unwrap(), 160, 20);
    host.app.set_office_runner(Arc::new(|_| {
        panic!("a missing target must not reach the GUI seam")
    }));
    if host.app.tab.projects.pending() {
        host.app.tab.projects.wait_for_result();
    }
    host.app.poll_projects();
    host.key('p');
    drain(&mut host);
    host
}

fn assert_launch_refused(host: &mut Sim, label: &str) {
    use crate::keymap::{Action, Context};
    for (key, msg) in [
        ('e', crate::i18n::Msg::NoFileToEdit),
        ('o', crate::i18n::Msg::NoFileToOpenApp),
    ] {
        host.key(key);
        drain(host);
        let mut launches = Vec::new();
        collect_host_launches(host, &mut launches);
        assert!(
            launches.is_empty(),
            "{label}: {key} must not record a launch: {launches:?}"
        );
        assert_eq!(
            host.app.flash.as_deref(),
            Some(crate::i18n::tr(host.app.lang, msg)),
            "{label}: no-target flash"
        );
        for action in [Action::RequestEdit, Action::OpenExternal] {
            assert!(
                !crate::ops::action_enabled(&host.app, Context::Canvas, action),
                "{label}: no e/o gate"
            );
        }
    }
    host.app.flash = None;
    let has_verb = |keys: &str| {
        keys.split(['/', ' ', ':'])
            .any(|key| key == "e" || key == "o")
    };
    assert!(
        !crate::ops::footer::tokens(&host.app, Context::Canvas)
            .iter()
            .any(|token| has_verb(
                token
                    .split_once(':')
                    .map_or(token.as_str(), |(keys, _)| keys)
            )),
        "{label}: no e/o footer hints"
    );
    assert!(
        !crate::ops::help::sections(&host.app)
            .iter()
            .flat_map(|section| &section.rows)
            .any(|(keys, _)| has_verb(keys)),
        "{label}: no e/o help hints"
    );
}

#[test]
fn project_deleted_rows_refuse_both_verbs_and_hide_hints() {
    if isolated("project_deleted_rows_refuse_both_verbs_and_hide_hints") {
        return;
    }
    for lang in ["en", "ja"] {
        let roots = worktree_repositories();
        std::fs::remove_file(roots.host.root.join("file.txt")).unwrap();
        let text = config(
            "git",
            lang,
            "vim",
            "relative",
            "right",
            "[editor]\ncommand='fixture-editor --line={line}'\n",
        );
        let mut host = launch_host(&roots.host.root, &text);
        let original = host_state(&host);
        assert!(
            original.target.is_some(),
            "deleted row keeps its data target"
        );
        assert_launch_refused(&mut host, "deleted changes row");
        let after = host_state(&host);
        assert_eq!(after.target, original.target);
        assert_eq!(after.selected, original.selected);
        host.enter();
        drain(&mut host);
        assert!(
            host.app.is_git_diff_preview(),
            "deleted rows still open their diff"
        );
        assert_launch_refused(&mut host, "deleted file source diff");
    }
}

#[test]
fn project_deleted_file_diff_refuses_launch_and_hides_hints() {
    if isolated("project_deleted_file_diff_refuses_launch_and_hides_hints") {
        return;
    }
    for lang in ["en", "ja"] {
        let roots = worktree_repositories();
        std::fs::remove_file(roots.host.root.join("file.txt")).unwrap();
        let text = config(
            "git",
            lang,
            "vim",
            "relative",
            "right",
            "[editor]\ncommand='fixture-editor --line={line}'\n",
        );
        let mut host = launch_host(&roots.host.root, &text);
        host.enter();
        drain(&mut host);
        assert!(host.app.is_git_diff_preview());
        assert_launch_refused(&mut host, "deleted file diff opened from a project");
    }
}

#[test]
fn project_detail_and_renamed_target_launch_audit() {
    if isolated("project_detail_and_renamed_target_launch_audit") {
        return;
    }
    for lang in ["en", "ja"] {
        let roots = worktree_repositories();
        std::fs::remove_file(roots.host.root.join("file.txt")).unwrap();
        let text = config(
            "git",
            lang,
            "vim",
            "relative",
            "right",
            "[editor]\ncommand='fixture-editor --line={line}'\n",
        );
        for graph in [false, true] {
            let mut host = launch_host(&roots.host.root, &text);
            host.press(KeyCode::Tab, KeyModifiers::NONE);
            if graph {
                host.press(KeyCode::Tab, KeyModifiers::NONE);
            }
            drain(&mut host);
            host.enter();
            drain(&mut host);
            assert!(matches!(
                host.app.project_content().as_deref(),
                Some(ProjectContent::Diff(_))
            ));
            assert!(
                host.app.project_row_target().is_none(),
                "log/graph detail exposes no file row target"
            );
            assert_launch_refused(
                &mut host,
                if graph {
                    "graph detail file"
                } else {
                    "log detail file"
                },
            );
        }
        let roots = worktree_repositories();
        super::vcs_log::worktree_git(&roots.host.root, &["mv", "file.txt", "renamed.txt"]);
        let mut host = launch_host(&roots.host.root, &text);
        assert_eq!(
            host.app.project_row_target(),
            Some(roots.host.root.join("renamed.txt")),
            "rename targets the new path, never its missing old path"
        );
        host.key('e');
        drain(&mut host);
        let mut launches = Vec::new();
        collect_host_launches(&mut host, &mut launches);
        assert_eq!(launches.len(), 1, "an existing renamed file still launches");
        assert!(launches[0]
            .target
            .as_deref()
            .unwrap()
            .ends_with("/renamed.txt"));
    }
}

#[test]
fn fresh_project_root_first_view_refuses_deleted_launch_target() {
    if isolated("fresh_project_root_first_view_refuses_deleted_launch_target") {
        return;
    }
    for lang in ["en", "ja"] {
        for switch in [false, true] {
            let roots = worktree_repositories();
            for linked in [&roots.host.linked, &roots.app.linked] {
                std::fs::remove_file(linked.join("file.txt")).unwrap();
            }
            let text = config(
                "git",
                lang,
                "vim",
                "relative",
                "right",
                "[editor]\ncommand='fixture-editor --line={line}'\n",
            );
            let mut steps = vec![];
            if switch {
                steps.extend([ch('w'), ch('/')]);
                steps.extend(typed("日本語"));
                steps.extend([key(KeyCode::Enter), key(KeyCode::Esc), key(KeyCode::Enter)]);
            }
            let first = steps.len();
            steps.extend([ch('e'), ch('o')]);
            let launches = Arc::new(Mutex::new(Vec::new()));
            let pair = if switch {
                (&roots.host.root, &roots.app.root)
            } else {
                (&roots.host.linked, &roots.app.linked)
            };
            run_roots_with(
                (pair.0, pair.1),
                "git",
                &text,
                steps,
                false,
                RunOptions {
                    worktrees: true,
                    launches: Some(launches.clone()),
                    ..Default::default()
                },
                |step, state, host, _| {
                    if step < first {
                        return;
                    }
                    assert_eq!(state.stack, ["changes"]);
                    let nav = host.app.tab.project_navigation.as_ref().unwrap();
                    let target = host
                        .app
                        .project_row_target()
                        .expect("deleted data target is retained");
                    assert_eq!(
                        host.app.project_runtimes[&nav.project].launch_targets[&nav.frame().unwrap().key].get(&target),
                        Some(&false),
                        "the first root view carries its missing-file launch decision before any refresh"
                    );
                    assert!(!crate::ops::action_enabled(
                        &host.app,
                        crate::keymap::Context::Canvas,
                        crate::keymap::Action::RequestEdit
                    ));
                    if step > first {
                        let msg = if step == first + 1 {
                            crate::i18n::Msg::NoFileToEdit
                        } else {
                            crate::i18n::Msg::NoFileToOpenApp
                        };
                        assert_eq!(
                            state.notice.as_deref(),
                            Some(crate::i18n::tr(host.app.lang, msg))
                        );
                    }
                },
            );
            assert!(launches.lock().unwrap().is_empty());
        }
    }
    report_cases();
}
