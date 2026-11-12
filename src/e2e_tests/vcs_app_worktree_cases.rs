// Worktree cases use the same uninterrupted loop and independently seeded repositories.

struct WorktreeRepository {
    root: std::path::PathBuf,
    linked: std::path::PathBuf,
}
struct WorktreeRepositories {
    host: WorktreeRepository,
    app: WorktreeRepository,
}
fn worktree_repositories() -> WorktreeRepositories {
    let parent = at_home(sandbox("vcs_app_worktrees"));
    let seed = |side| {
        let dir = parent.join(side);
        std::fs::create_dir_all(&dir).unwrap();
        let (root, linked, _) = super::vcs_log::seed_worktrees(&dir);
        WorktreeRepository { root, linked }
    };
    WorktreeRepositories {
        host: seed("host"),
        app: seed("apps"),
    }
}
fn run_worktrees(
    roots: &WorktreeRepositories,
    linked: bool,
    text: &str,
    steps: Vec<Step>,
    check: impl FnMut(usize, &ProjectStateSnapshot, &Sim, &[String]),
) {
    let pair = if linked {
        (&roots.host.linked, &roots.app.linked)
    } else {
        (&roots.host.root, &roots.app.root)
    };
    run_roots_with(
        (pair.0, pair.1),
        "git",
        text,
        steps,
        false,
        RunOptions {
            worktrees: true,
            ..Default::default()
        },
        check,
    );
}

fn check_switched_tree(host: &mut Sim, app: &ProjectStateSnapshot, root: &Path, app_root: &Path) {
    // §4-1:220 has no Tree in the standalone; §7-5:959 opens the new first screen.
    assert!(matches!(host.app.tab.mode, crate::app::Mode::Tree));
    let relative = host
        .app
        .tab
        .root
        .strip_prefix(root.parent().unwrap())
        .unwrap();
    assert_eq!(
        app.root.as_deref(),
        Some(app_root.parent().unwrap().join(relative).as_path()),
        "shared new root path"
    );
    assert_eq!(app.open_dir, app.root, "new open_dir follows root");
    assert_eq!(
        app.stack,
        ["changes"],
        "standalone opens the first project screen"
    );
    let area = host.term.backend().buffer().area;
    let mut expected = Sim::with_config_sized(
        &host.app.tab.root,
        host.app.cfg.clone(),
        area.width,
        area.height,
    );
    if expected.app.tab.projects.pending() {
        expected.app.tab.projects.wait_for_result();
    }
    expected.app.poll_projects();
    expected.app.flash = host.app.flash.clone();
    compare_frames(&expected.project_frame(), &host.project_frame())
        .unwrap_or_else(|e| panic!("explicit host Tree after SwitchRoot: {e}"));
    let notice = host.app.flash.clone();
    if host.app.tab.projects.pending() {
        host.app.tab.projects.wait_for_result();
    }
    host.app.poll_projects();
    // Reopen only the host's project surface after checking its documented Tree outcome.
    host.key('p');
    drain(host);
    host.app.flash = notice;
}

fn worktree_failure_and_diff_cases() {
    for lang in ["en", "ja"] {
        for readonly in [false, true] {
            let roots = worktree_repositories();
            for linked in [&roots.host.linked, &roots.app.linked] {
                let body = (0..70)
                    .map(|i| format!("feature {i} 日本語 {}\n", "long ".repeat(30)))
                    .collect::<String>();
                std::fs::write(linked.join("file.txt"), body).unwrap();
                super::vcs_log::commit_worktree(linked);
            }
            for root in [&roots.host.root, &roots.app.root] {
                std::fs::write(
                    root.join("file.txt"),
                    (0..70)
                        .map(|i| format!("uncommitted {i} 日本語 {}\n", "long ".repeat(30)))
                        .collect::<String>(),
                )
                .unwrap();
            }
            let text = config(
                "git",
                lang,
                "vim",
                "relative",
                "right",
                &format!("[plugins.git]\nread_only={readonly}\n[keys.text]\nD='navigate:down'\n"),
            );
            for linked in [false, true] {
                let mut steps = vec![ch('w')];
                if readonly {
                    steps.push(ch('c'));
                }
                if linked {
                    steps.push(ch('/'));
                    steps.extend(typed("日本語"));
                    steps.extend([key(KeyCode::Enter), key(KeyCode::Esc)]);
                }
                steps.extend([
                    ch('d'),
                    ch('D'),
                    ch('j'),
                    ch('l'),
                    ch('s'),
                    ch('G'),
                    ch('g'),
                    key(KeyCode::PageDown),
                    key(KeyCode::PageUp),
                    Step::Resize(48, 9),
                    Step::Resize(160, 20),
                    ch('q'),
                ]);
                run_worktrees(&roots, false, &text, steps, |step, state, _, _| {
                    assert!(
                        state.prompt.is_none(),
                        "navigation and diff do not create write prompts"
                    );
                    if step == 3 + usize::from(readonly) + if linked { 6 } else { 0 } {
                        assert!(
                            state.scroll > 0,
                            "the remapped D key scrolls the worktree diff"
                        );
                    }
                });
            }
        }
    }
    for switch in [KeyCode::Enter, KeyCode::Char('l')] {
        let roots = worktree_repositories();
        let text = config("git", "en", "vim", "relative", "right", "");
        let mut steps = vec![ch('w'), ch('/')];
        steps.extend(typed("日本語"));
        steps.extend([
            key(KeyCode::Enter),
            key(KeyCode::Esc),
            key(switch),
            ch('j'),
            ch('k'),
        ]);
        let mut before = None;
        run_roots_with(
            (&roots.host.root, &roots.app.root),
            "git",
            &text,
            steps,
            false,
            RunOptions {
                worktrees: true,
                remove_switch_target: true,
                ..Default::default()
            },
            |step, state, _, _| {
                if step == 7 {
                    before = Some(state.clone());
                }
                if step == 8 {
                    let before = before.as_ref().unwrap();
                    assert_eq!(state.root, before.root, "failed switch keeps root");
                    assert_eq!(
                        state.open_dir, before.open_dir,
                        "failed switch keeps open_dir"
                    );
                    assert_eq!(state.stack, before.stack, "failed switch keeps frame");
                    assert_eq!(
                        state.selected, before.selected,
                        "failed switch keeps selection"
                    );
                    assert_eq!(
                        state.target, before.target,
                        "failed switch keeps launch target"
                    );
                    assert!(state.notice.as_deref().unwrap().starts_with("not found: "));
                }
                if step == 9 {
                    assert_ne!(
                        state.cursor,
                        before.as_ref().unwrap().cursor,
                        "input continues after a failed switch"
                    );
                }
            },
        );
    }
    let roots = worktree_repositories();
    let (_, host) = super::vcs_log::seed_bare_worktrees(roots.host.root.parent().unwrap());
    let (_, app) = super::vcs_log::seed_bare_worktrees(roots.app.root.parent().unwrap());
    for lang in ["en", "ja"] {
        let text = config("git", lang, "vim", "relative", "right", "");
        run_roots_with(
            (&host, &app),
            "git",
            &text,
            vec![
                ch('w'),
                ch('g'),
                key(KeyCode::Enter),
                ch('l'),
                ch('d'),
                Step::Resize(160, 20),
            ],
            false,
            RunOptions {
                worktrees: true,
                ..Default::default()
            },
            |_, state, _, _| {
                assert_eq!(state.root.as_deref(), Some(Path::new("<repo>/bare-linked")));
            },
        );
        for query in ["main", "prunable"] {
            let mut steps = vec![ch('w'), ch('/')];
            steps.extend(typed(query));
            steps.extend([
                key(KeyCode::Enter),
                key(KeyCode::Esc),
                key(KeyCode::Enter),
                ch('l'),
                ch('d'),
            ]);
            run_worktrees(&roots, false, &text, steps, |_, state, _, _| {
                assert_eq!(state.root.as_deref(), Some(Path::new("<repo>/main")));
            });
        }
    }
}

fn project_launch_cases() {
    let roots = worktree_repositories();
    let text = config(
        "git",
        "en",
        "vim",
        "relative",
        "right",
        "[editor]\ncommand='fixture-editor --line={line}'\n",
    );
    let launches = Arc::new(Mutex::new(Vec::new()));
    run_roots_with(
        (&roots.host.root, &roots.app.root),
        "git",
        &text,
        vec![ch('e'), ch('w'), ch('e'), ch('j'), ch('e')],
        false,
        RunOptions {
            worktrees: true,
            launches: Some(launches.clone()),
            ..Default::default()
        },
        |_, state, _, _| {
            assert!(state.prompt.is_none());
        },
    );
    assert!(
        launches
            .lock()
            .unwrap()
            .iter()
            .all(|launch| launch.target.as_deref() == Some("<repo>/detached")),
        "only the selected worktree directory launches; targetless screens do not launch the root"
    );
    assert_eq!(launches.lock().unwrap().len(), 1);

    for lang in ["en", "ja"] {
        let roots = worktree_repositories();
        for root in [&roots.host.root, &roots.app.root] {
            std::fs::write(root.join("file.txt"), "changed\n").unwrap();
        }
        let text = config(
            "git",
            lang,
            "vim",
            "relative",
            "right",
            "[editor]\ncommand='fixture-editor --line={line}'\n",
        )
        .replace(
            "diff='unified'",
            "diff='unified'\ntool='fixture-tool --git'",
        );
        let launches = Arc::new(Mutex::new(Vec::new()));
        run_roots_with(
            (&roots.host.root, &roots.app.root),
            "git",
            &text,
            vec![
                ch('e'),
                ch('!'),
                key(KeyCode::Tab),
                key(KeyCode::Enter),
                ch('e'),
                ch('!'),
                ch('q'),
                ch('w'),
                ch('j'),
                ch('e'),
                ch('!'),
            ],
            false,
            RunOptions {
                worktrees: true,
                launches: Some(launches.clone()),
                ..Default::default()
            },
            |_, state, _, _| {
                assert!(state.prompt.is_none());
            },
        );
        let launches = launches.lock().unwrap();
        assert_eq!(
            launches
                .iter()
                .filter(|r| r.program == "fixture-editor")
                .count(),
            2
        );
        assert_eq!(
            launches
                .iter()
                .filter(|r| r.program == "fixture-tool")
                .count(),
            // Old hub: `!` is bound only in the changes screen's table, so of the three `!`
            // presses (changes, a pushed detail, worktrees) only the first launches the tool.
            1
        );
        assert!(launches.iter().all(|r| r.backend == "git"));
    }
    for lang in ["en", "ja"] {
        let roots = worktree_repositories();
        for root in [&roots.host.root, &roots.app.root] {
            std::fs::remove_file(root.join("file.txt")).unwrap();
        }
        let text = config(
            "git",
            lang,
            "vim",
            "relative",
            "right",
            "[editor]\ncommand='fixture-editor --line={line}'\n",
        );
        let launches = Arc::new(Mutex::new(Vec::new()));
        run_roots_with(
            (&roots.host.root, &roots.app.root),
            "git",
            &text,
            vec![ch('e'), ch('o')],
            false,
            RunOptions {
                worktrees: true,
                launches: Some(launches.clone()),
                ..Default::default()
            },
            |step, state, host, _| {
                assert!(
                    state.target.is_some(),
                    "the existing row-data contract retains deleted paths"
                );
                if step == 0 {
                    return;
                }
                let message = if step == 1 {
                    crate::i18n::Msg::NoFileToEdit
                } else {
                    crate::i18n::Msg::NoFileToOpenApp
                };
                assert_eq!(
                    state.notice.as_deref(),
                    Some(crate::i18n::tr(host.app.lang, message))
                );
                for action in [
                    crate::keymap::Action::RequestEdit,
                    crate::keymap::Action::OpenExternal,
                ] {
                    assert!(
                        !crate::ops::action_enabled(
                            &host.app,
                            crate::keymap::Context::Canvas,
                            action
                        ),
                        "deleted row does not enable e/o"
                    );
                }
            },
        );
        assert!(launches.lock().unwrap().is_empty());
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct LaunchRecord {
    program: String,
    args: Vec<String>,
    target: Option<String>,
    line: Option<usize>,
    backend: String,
    cwd: String,
}
impl LaunchRecord {
    fn new(attempt: &konoma_plugin::launch::Attempt, cwd: &Path, backend: &str) -> Self {
        let editor = attempt.prog == "fixture-editor";
        Self {
            program: attempt.prog.clone(),
            args: attempt.args.clone(),
            target: editor.then(|| attempt.args.last().unwrap().clone()),
            line: editor.then(|| {
                attempt.args[0]
                    .strip_prefix("--line=")
                    .unwrap()
                    .parse()
                    .unwrap()
            }),
            backend: backend.into(),
            cwd: cwd.to_string_lossy().into_owned(),
        }
    }
    fn normalized(&self, root: &Path) -> Self {
        let mut record = self.clone();
        let root = root.to_str().unwrap();
        for value in record
            .args
            .iter_mut()
            .chain(record.target.iter_mut())
            .chain(std::iter::once(&mut record.cwd))
        {
            *value = value.replace(root, "<repo>");
        }
        record
    }
}
fn collect_host_launches(host: &mut Sim, records: &mut Vec<LaunchRecord>) -> bool {
    let backend = match host.app.diff_vcs() {
        crate::vcs::VcsKind::Git => "git",
        crate::vcs::VcsKind::Jj => "jj",
    };
    if let Some((path, line)) = host.app.take_pending_edit() {
        let argv = host.app.cfg.editor.resolve(&path, line);
        records.push(LaunchRecord::new(
            &konoma_plugin::launch::Attempt {
                opener: konoma_plugin::launch::Opener::Declared,
                prog: argv[0].clone(),
                args: argv[1..].to_vec(),
            },
            path.parent().unwrap(),
            backend,
        ));
    }
    if let Some((attempts, cwd)) = host.app.take_pending_terminal() {
        assert_eq!(attempts.len(), 1);
        records.push(LaunchRecord::new(&attempts[0], &cwd, backend));
    }
    if host.app.take_launch_git_tool() {
        let plan = crate::git_tool_plan(&host.app).unwrap();
        records.push(LaunchRecord::new(&plan.attempt, &plan.cwd, backend));
        return true;
    }
    false
}

#[test]
fn worktree_create_switch_failure_and_diff_match_host() {
    if isolated("worktree_create_switch_failure_and_diff_match_host") {
        return;
    }
    for lang in ["en", "ja"] {
        for linked in [false, true] {
            let roots = worktree_repositories();
            let text = config(
                "git",
                lang,
                "vim",
                "relative",
                "right",
                "[keys.text]\nD='navigate:down'\n",
            );
            let mut steps = vec![ch('w'), Step::Resize(160, 20), ch('j'), ch('k')];
            for query in ["日本語", "LINKED", "no-match"] {
                steps.push(ch('/'));
                steps.extend(typed(query));
                steps.extend([key(KeyCode::Enter), key(KeyCode::Esc)]);
            }
            steps.extend([Step::Resize(48, 9), ch('G'), ch('g'), Step::Resize(160, 20)]);
            run_worktrees(&roots, linked, &text, steps, |_, _, _, _| {});
        }
        for (name, directory, existing) in [
            ("fresh", "../", false),
            ("  feat/nested  ", "../", false),
            ("existing", "../placed", true),
            ("filtered", "../", false),
        ] {
            let roots = worktree_repositories();
            if existing {
                for root in [&roots.host.root, &roots.app.root] {
                    super::vcs_log::worktree_git(root, &["branch", name]);
                }
            }
            let text = config(
                "git",
                lang,
                "vim",
                "relative",
                "right",
                &format!("[plugins.git]\nworktree_dir='{directory}'\n"),
            );
            let mut steps = vec![ch('w')];
            if name == "filtered" {
                steps.push(ch('/'));
                steps.extend(typed("no-match"));
                steps.push(key(KeyCode::Enter));
            }
            steps.push(ch('c'));
            steps.extend(typed(name));
            steps.extend([
                Step::Write(
                    KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
                    "worktree_create",
                ),
                ch('w'),
            ]);
            run_worktrees(&roots, true, &text, steps, |_, _, _, _| {});
        }
        let roots = worktree_repositories();
        let text = config("git", lang, "vim", "relative", "right", "");
        run_worktrees(
            &roots,
            false,
            &text,
            vec![
                ch('w'),
                ch('c'),
                ch(' '),
                Step::Write(
                    KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
                    "worktree_create",
                ),
                ch('c'),
                ch('m'),
                ch('a'),
                ch('i'),
                ch('n'),
                Step::Write(
                    KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
                    "worktree_create",
                ),
                ch('j'),
                ch('k'),
                ch('j'),
            ],
            |step, state, _, _| {
                if step == 4 {
                    assert!(
                        state.prompt.is_none(),
                        "blank names close the prompt like konoma"
                    );
                    assert!(state.notice.is_some());
                }
                if step == 10 {
                    // The existing worktree test explicitly requires this failed prompt to close.
                    assert!(
                        state.prompt.is_none(),
                        "keep konoma's worktree failure behavior"
                    );
                    assert!(state.notice.is_some());
                }
            },
        );
        for switch in [KeyCode::Enter, KeyCode::Char('l')] {
            let roots = worktree_repositories();
            let mut steps = vec![ch('w'), ch('/')];
            steps.extend(typed("日本語"));
            steps.extend([key(KeyCode::Enter), key(KeyCode::Esc), key(switch), ch('w')]);
            run_worktrees(&roots, false, &text, steps, |_, _, _, _| {});
        }
        let roots = worktree_repositories();
        let mut steps = vec![ch('w'), ch('/')];
        for letter in ['t', 'c', 'x', 'w', 'q', 'a', 'd'] {
            steps.push(Step::Key(KeyEvent::new(
                KeyCode::Char(letter),
                KeyModifiers::CONTROL,
            )));
        }
        steps.extend([
            ch('日'),
            Step::Key(KeyEvent::new(KeyCode::Char('Q'), KeyModifiers::SHIFT)),
            Step::Key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::ALT)),
            key(KeyCode::Backspace),
            key(KeyCode::Enter),
            key(KeyCode::Esc),
        ]);
        run_worktrees(&roots, false, &text, steps, |step, state, _, _| {
            if (2..=9).contains(&step) {
                assert!(state.query.is_empty());
                assert!(state.filtering);
            }
            if step == 11 || step == 12 {
                assert_eq!(state.query, "日Q");
            }
            if step == 13 || step == 14 {
                assert_eq!(state.query, "日");
            }
        });
        host_only_worktree_tab(&roots, &text);
    }
    worktree_failure_and_diff_cases();
    report_cases();
}

fn host_only_worktree_tab(roots: &WorktreeRepositories, text: &str) {
    let mut host = Sim::with_config_sized(&roots.host.root, toml::from_str(text).unwrap(), 90, 14);
    if host.app.tab.projects.pending() {
        host.app.tab.projects.wait_for_result();
    }
    host.app.poll_projects();
    host.key('p');
    drain(&mut host);
    host.key('w');
    drain(&mut host);
    host.press(KeyCode::Char('t'), KeyModifiers::CONTROL);
    drain(&mut host);
    assert_eq!(
        host.app.tab_count(),
        1,
        "current worktree has no Ctrl-t target"
    );
    host.key('/');
    host.keys("日本語");
    host.enter();
    host.esc();
    drain(&mut host);
    let original = host_state(&host);
    host.press(KeyCode::Char('t'), KeyModifiers::CONTROL);
    drain(&mut host);
    assert_eq!(
        host.app.tab_count(),
        2,
        "Ctrl-t is host-only (§4-1:220 has no standalone tabs)"
    );
    assert_eq!(host.app.tab.root, roots.host.linked);
    assert_eq!(host.app.tab.open_dir, roots.host.linked);
    assert_eq!(host.app.tab.mode, crate::app::Mode::Tree);
    host.app.tab_goto(0);
    drain(&mut host);
    assert_eq!(
        host_state(&host),
        original,
        "host-only Ctrl-t preserves the original worktree frame and selection"
    );
}

#[test]
fn root_replacement_rejects_old_jobs_and_preserves_failure_state() {
    if isolated("root_replacement_rejects_old_jobs_and_preserves_failure_state") {
        return;
    }
    continuous_root_replacement_cases();
    report_cases();
}

#[test]
fn project_launch_records_current_target_line_backend_and_cwd() {
    if isolated("project_launch_records_current_target_line_backend_and_cwd") {
        return;
    }
    project_launch_cases();
    if !crate::vcs::jj::available() {
        report_cases();
        eprintln!("NOT RUN: project launches: jj unavailable; 0 jj sessions run; 6 jj-only/colocated Git/jj sessions not run");
    }
    if !crate::vcs::jj::available() {
        return;
    }
    jj_launch_cases();
    report_cases();
}

fn jj_launch_cases() {
    for (colocated, id) in [(false, "jj"), (true, "git"), (true, "jj")] {
        for lang in ["en", "ja"] {
            let parent = at_home(sandbox("vcs_app_jj_launch"));
            let roots = WriteRepositories {
                host: parent.join("host/repo"),
                app: parent.join("apps/repo"),
            };
            for root in [&roots.host, &roots.app] {
                super::vcs_changes::seed_jj_write_repository(root, colocated);
            }
            let text = config(id, lang, "vim", "relative", "right", "[editor]\ncommand='fixture-editor --line={line}'\n[jj]\ntool='fixture-tool --jj'\n")
                .replace("diff='unified'", "diff='unified'\ntool='fixture-tool --git'");
            let launches = Arc::new(Mutex::new(Vec::new()));
            run_roots_with(
                (&roots.host, &roots.app),
                id,
                &text,
                vec![ch('e'), ch('!'), ch('e'), ch('!')],
                false,
                RunOptions {
                    launches: Some(launches.clone()),
                    ..Default::default()
                },
                |_, state, _, _| {
                    assert!(
                        !state.stack.iter().any(|s| s == "worktrees"),
                        "jj declares no worktree/workspace screen"
                    );
                },
            );
            let launches = launches.lock().unwrap();
            assert_eq!(
                launches.len(),
                4,
                "two file editor and two backend tool launches"
            );
            assert!(launches.iter().all(|r| r.backend == id));
            assert!(launches
                .iter()
                .filter(|r| r.program == "fixture-tool")
                .all(|r| r.args == [format!("--{id}")]));
        }
    }
    eprintln!("RUN: project launches: 6 jj-only/colocated Git/jj sessions; workspace screens absent from jj's declaration");
}
