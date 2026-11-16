// Write cases share the uninterrupted U8b/U8c driver and its full-frame assertions.

type WriteCheck = Box<dyn Fn(&Path, &ProjectStateSnapshot)>;

#[derive(Default)]
struct WriteScript {
    steps: Vec<Step>,
    checks: std::collections::BTreeMap<usize, Vec<WriteCheck>>,
}

impl WriteScript {
    fn key(&mut self, code: KeyCode) {
        self.steps.push(key(code));
    }
    fn act(&mut self, code: KeyCode, action: &'static str) {
        self.steps
            .push(Step::Write(KeyEvent::new(code, KeyModifiers::NONE), action));
    }
    fn text(&mut self, value: &str) {
        self.steps.extend(typed(value));
    }
    fn before(&mut self, code: KeyCode, f: impl Fn(&Path) + 'static) {
        self.steps.push(Step::BeforeKey(
            Box::new(f),
            KeyEvent::new(code, KeyModifiers::NONE),
        ));
    }
    fn prompt(&mut self, kind: &'static str, text: &'static str) {
        self.expect(move |_, state| {
            let prompt = state.prompt.as_ref().expect("prompt must remain open");
            assert_eq!(prompt.kind, kind);
            assert_eq!(prompt.buffer, text);
        });
    }
    fn notice(&mut self, expected: &str) {
        let expected = expected.to_owned();
        self.expect(move |_, state| assert_eq!(state.notice.as_deref(), Some(expected.as_str())));
    }
    fn message(&mut self, lang: &str, msg: crate::i18n::Msg) {
        self.notice(crate::i18n::tr(
            if lang == "ja" {
                crate::i18n::Lang::Jp
            } else {
                crate::i18n::Lang::En
            },
            msg,
        ));
    }
    fn expect(&mut self, check: impl Fn(&Path, &ProjectStateSnapshot) + 'static) {
        self.checks
            .entry(self.steps.len())
            .or_default()
            .push(Box::new(check));
    }
    fn select(&mut self, value: &str) {
        self.key(KeyCode::Char('/'));
        self.text(value);
        self.key(KeyCode::Enter);
        self.expect(|_, state| assert!(state.selected.is_some(), "filter must select a real row"));
        self.key(KeyCode::Esc);
    }
}

struct WriteRepositories {
    host: std::path::PathBuf,
    app: std::path::PathBuf,
}

fn write_repositories(changed: bool) -> WriteRepositories {
    let parent = at_home(sandbox("vcs_app_writes"));
    let roots = WriteRepositories {
        host: parent.join("host/repo"),
        app: parent.join("apps/repo"),
    };
    for root in [&roots.host, &roots.app] {
        super::vcs_changes::seed_write_repository(root, changed);
    }
    roots
}

fn git_index_bytes(root: &Path, path: &str) -> Option<Vec<u8>> {
    let repo = konoma_vcs_core::git2::Repository::open(root).unwrap();
    let entry = repo.index().unwrap().get_path(Path::new(path), 0)?;
    let bytes = repo.find_blob(entry.id).unwrap().content().to_vec();
    Some(bytes)
}

fn normalize_write_state(state: &mut ProjectStateSnapshot, root: &Path) {
    if let Some(diff) = &mut state.diff {
        if let Ok(path) = diff.path.strip_prefix(root) { diff.path = Path::new("<repo>").join(path); }
    }
    for path in [&mut state.root, &mut state.open_dir, &mut state.target].into_iter().flatten() {
        if let Ok(relative) = path.strip_prefix(root) {
            *path = Path::new("<repo>").join(relative);
        }
    }
    let normalize = |s: &mut String| {
        *s = s.replace(root.to_str().unwrap(), "<repo>");
        let home = std::path::PathBuf::from(std::env::var_os("HOME").unwrap());
        if let Ok(relative) = root.strip_prefix(home) {
            *s = s.replace(&format!("~/{}", relative.display()), "<repo>");
        }
    };
    for selection in state.stack_selection.iter_mut().flatten() {
        normalize(selection);
    }
    for row in state
        .selected
        .iter_mut()
        .chain(state.marked.iter_mut())
        .chain(state.rows.iter_mut().map(|(r, _)| r))
        .chain(state.visible.iter_mut())
    {
        normalize(&mut row.value);
        if let Some(name) = &mut row.name {
            normalize(name);
        }
    }
    if let Some(notice) = &mut state.notice {
        normalize(notice);
    }
}

fn normalize_write_frame(frame: &mut Buffer, root: &Path) {
    // Independent repositories have equal-width identity components. Preserve every cell's
    // position and style; translate only that exact component, including in clipped paths.
    let component = root
        .parent()
        .unwrap()
        .file_name()
        .unwrap()
        .to_str()
        .unwrap();
    assert!(matches!(component, "host" | "apps"));
    let needle: Vec<_> = format!("/{component}/")
        .chars()
        .map(|c| c.to_string())
        .collect();
    for y in 0..frame.area.height {
        for x in 0..frame.area.width.saturating_sub(5) {
            if needle
                .iter()
                .enumerate()
                .all(|(i, c)| frame[(x + i as u16, y)].symbol() == c)
            {
                for (i, c) in "side".chars().enumerate() {
                    frame[(x + 1 + i as u16, y)].set_symbol(&c.to_string());
                }
            }
        }
        let parent = root.parent().unwrap().parent().unwrap();
        let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
        let parent_names = [
            parent.to_string_lossy().into_owned(),
            konoma_host_core::paths::home_relative_to(parent, home.as_deref()),
        ];
        for x in frame.area.width.saturating_sub(8)..frame.area.width {
            let before: String = (0..x).map(|x| frame[(x, y)].symbol()).collect();
            if !parent_names.iter().any(|parent| before.ends_with(parent)) {
                continue;
            }
            // A narrow body can clip the identity component before its closing slash.
            // Translate only that known path prefix, preserving clipping and all cell styles.
            for length in (2..needle.len()).rev() {
                if usize::from(x) + length > usize::from(frame.area.width) {
                    continue;
                }
                let prefix_matches = needle[..length].iter().enumerate()
                    .all(|(i, c)| frame[(x + i as u16, y)].symbol() == c);
                // What follows the clip is blank padding, then (inside a project frame) the
                // frame's right border in the last column.
                let last = frame.area.width - 1;
                let padding = (x + length as u16..frame.area.width).all(|x| {
                    let symbol = frame[(x, y)].symbol();
                    symbol.trim().is_empty() || (x == last && symbol == "│")
                });
                if prefix_matches && padding {
                    for (i, c) in "side".chars().take(length - 1).enumerate() {
                        frame[(x + 1 + i as u16, y)].set_symbol(&c.to_string());
                    }
                    break;
                }
            }
        }
    }
}

/// The identity component can be clipped by the body anywhere in `/host/` or `/apps/`, depending
/// on how many digits the process id and counter in the sandbox name have. Inside a project frame
/// the clip is followed by the frame's right border; both sides must still normalize equal.
#[test]
fn clipped_identity_component_before_the_frame_border_normalizes() {
    // Same naming as the real sandboxes; nothing is created on disk (the path is only text).
    let dir = crate::test_support::unique_tmp("konoma_e2e_x");
    let parent = dir.as_path().to_path_buf();
    let build = |component: &str, clip: usize, gap: u16, border: bool| {
        let root = parent.join(component).join("repo");
        let text = format!("{}/{component}/", parent.display());
        let clipped: String = text.chars().take(text.chars().count() - 6 + clip).collect();
        let width = 1 + clipped.chars().count() as u16 + gap + 1;
        let mut frame = Buffer::empty(Rect::new(0, 0, width, 3));
        frame.set_string(1, 1, &clipped, Style::default());
        if border {
            frame.set_string(width - 1, 1, "│", Style::default());
        }
        normalize_write_frame(&mut frame, &root);
        frame
    };
    // Lengths 2..=5 are the clips `/h`, `/ho`, `/hos`, `/host` that the border can follow.
    for clip in 2..=5 {
        for gap in 0..=2 {
            let host = build("host", clip, gap, true);
            let app = build("apps", clip, gap, true);
            compare_frames(&host, &app)
                .unwrap_or_else(|e| panic!("clip {clip} gap {gap} before the border: {e}"));
            let host = build("host", clip, gap, false);
            let app = build("apps", clip, gap, false);
            compare_frames(&host, &app)
                .unwrap_or_else(|e| panic!("clip {clip} gap {gap} without a border: {e}"));
        }
    }
}

fn same_write_value<T: std::fmt::Debug + PartialEq>(
    host: T,
    app: T,
    label: &str,
) -> Result<(), String> {
    if host == app {
        Ok(())
    } else {
        Err(format!("{label}: host={host:?}, app={app:?}"))
    }
}

fn compare_dispatched_writes(
    host: &[String],
    app: &[String],
    expected: &[String],
) -> Result<(), String> {
    for (side, actual) in [("host", host), ("app", app)] {
        same_write_value(actual, expected, &format!("{side} dispatched writes"))?;
    }
    Ok(())
}

fn assert_write_repositories(host: &Path, app: &Path, id: &str, step: usize) {
    compare_write_repositories(host, app, id, step).unwrap_or_else(|e| panic!("{e}"));
}

fn compare_write_repositories(
    host: &Path,
    app: &Path,
    id: &str,
    step: usize,
) -> Result<(), String> {
    if id == "jj" {
        same_write_value(
            super::vcs_changes::write_jj_state(host),
            super::vcs_changes::write_jj_state(app),
            &format!("step {step}: jj repository state"),
        )?;
        if !host.join(".git").exists() {
            return Ok(());
        }
    }
    use super::vcs_changes::write_git_output as git_output;
    for args in [
        vec!["status", "--porcelain=v1"],
        vec!["ls-files", "--stage"],
        vec!["log", "-1", "--format=%s"],
        vec!["branch", "--show-current"],
        vec!["for-each-ref", "--format=%(refname:short)", "refs/heads"],
    ] {
        same_write_value(
            git_output(host, &args),
            git_output(app, &args),
            &format!("step {step}: repository {args:?}"),
        )?;
    }
    let paths: std::collections::BTreeSet<_> = [host, app]
        .into_iter()
        .flat_map(|root| {
            git_output(
                root,
                &[
                    "ls-files",
                    "--cached",
                    "--others",
                    "--exclude-standard",
                    "-z",
                ],
            )
            .split('\0')
            .filter(|p| !p.is_empty())
            .map(str::to_owned)
            .collect::<Vec<_>>()
        })
        .chain(
            [
                "a-modified",
                "b-new",
                "c-deleted",
                "d-old",
                "d-renamed",
                "sub/日本語.txt",
            ]
            .map(str::to_owned),
        )
        .collect();
    for path in paths {
        same_write_value(
            git_index_bytes(host, &path),
            git_index_bytes(app, &path),
            &format!("step {step}: index bytes {path}"),
        )?;
        let bytes = |root: &Path| match std::fs::read(root.join(&path)) {
            Ok(bytes) => Some(bytes),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => panic!("worktree read {path}: {e}"),
        };
        same_write_value(
            bytes(host),
            bytes(app),
            &format!("step {step}: worktree bytes {path}"),
        )?;
    }
    Ok(())
}

fn write_oracle_negative_controls() {
    let stage = vec!["stage".to_owned()];
    assert!(
        compare_dispatched_writes(&stage, &[], &stage).is_err(),
        "write counter must inspect app side"
    );
    assert!(
        compare_dispatched_writes(&[], &stage, &stage).is_err(),
        "write counter must inspect host side"
    );
    let roots = write_repositories(false);
    assert_write_repositories(&roots.host, &roots.app, "git", 0);
    for (side, root) in [("host", &roots.host), ("app", &roots.app)] {
        std::fs::write(root.join("a-modified"), "negative control\n").unwrap();
        assert!(
            compare_write_repositories(&roots.host, &roots.app, "git", 0).is_err(),
            "repository comparison must inspect {side} side"
        );
        std::fs::write(root.join("a-modified"), "base a-modified\n").unwrap();
    }
    assert_write_repositories(&roots.host, &roots.app, "git", 0);
}

fn run_writes(roots: &WriteRepositories, id: &str, text: &str, script: WriteScript) {
    let WriteScript { steps, checks } = script;
    run_roots(
        (&roots.host, &roots.app),
        id,
        text,
        steps,
        false,
        |step, state, _, _| {
            if let Some(checks) = checks.get(&step) {
                for check in checks {
                    check(&roots.host, state);
                    check(&roots.app, state);
                }
            }
        },
    );
}

fn jj_write_cases() {
    let mut runs = 0;
    for lang in ["ja", "en"] {
        for colocated in [false, true] {
            for (confirm, read_only) in [(true, false), (false, false), (true, true), (false, true)]
            {
                let parent = at_home(sandbox("vcs_app_jj_writes"));
                let roots = WriteRepositories {
                    host: parent.join("host/repo"),
                    app: parent.join("apps/repo"),
                };
                for root in [&roots.host, &roots.app] {
                    super::vcs_changes::seed_jj_write_repository(root, colocated);
                }
                let text = config(
                    "jj",
                    lang,
                    "vim",
                    "relative",
                    "right",
                    &format!("[plugins.jj]\nconfirm_sync={confirm}\nread_only={read_only}\n"),
                );
                let mut script = WriteScript::default();
                // Reach both backend chips through a real diff, including read-only projects.
                script.key(KeyCode::Enter);
                script.key(KeyCode::Char('q'));
                if read_only {
                    script.key(KeyCode::Char('s'));
                    script.expect(|root, state| {
                        assert!(
                            state.prompt.is_none(),
                            "jj read-only must refuse before prompting"
                        );
                        assert_eq!(
                            super::vcs_changes::write_jj_state(root)[0],
                            "base\n",
                            "jj read-only must not snapshot"
                        );
                    });
                } else if confirm {
                    for cancel in [KeyCode::Char('n'), KeyCode::Esc] {
                        script.key(KeyCode::Char('s'));
                        script.prompt("confirm", "");
                        script.key(KeyCode::Enter);
                        script.prompt("confirm", "");
                        script.key(cancel);
                        script.expect(|root, state| {
                            assert!(state.prompt.is_none());
                            assert_eq!(super::vcs_changes::write_jj_state(root)[0], "base\n");
                        });
                    }
                    script.key(KeyCode::Char('s'));
                    script.act(KeyCode::Char('Y'), "sync");
                    script.expect(|root, state| {
                        assert!(state.prompt.is_none());
                        assert_eq!(super::vcs_changes::write_jj_state(root)[0], "changed\n");
                    });
                    script.key(KeyCode::Char('s'));
                    script.act(KeyCode::Char('y'), "sync");
                } else {
                    script.act(KeyCode::Char('s'), "sync");
                    script.expect(|root, state| {
                        assert!(
                            state.prompt.is_none(),
                            "confirm_sync=false snapshots directly"
                        );
                        assert_eq!(super::vcs_changes::write_jj_state(root)[0], "changed\n");
                    });
                }
                eprintln!(
                    "RUN: jj writes: {lang}, colocated={colocated}, confirm={confirm}, read_only={read_only}"
                );
                run_writes(&roots, "jj", &text, script);
                runs += 1;
            }
        }
    }
    eprintln!("U8d jj: {runs} sessions run; 0 not run");
}

fn report_write_cases() {
    TALLY.with(|t| {
        let (runs, fences) = t.get();
        eprintln!("U8d: {runs} sessions, {fences} frame/state/repository/dispatch fences");
    });
}

#[test]
fn git_writes_and_repository_results_match_host() {
    if isolated("git_writes_and_repository_results_match_host") {
        return;
    }
    write_oracle_negative_controls();
    for lang in ["ja", "en"] {
        for style in ["relative", "home", "full"] {
            for file in [
                "a-modified",
                "b-new",
                "c-deleted",
                "d-renamed",
                "sub/日本語.txt",
            ] {
                let roots = write_repositories(true);
                let text = config(
                    "git",
                    lang,
                    "vim",
                    style,
                    "right",
                    "[keys.text]\nJ='navigate:down'\n",
                );
                let mut script = WriteScript::default();
                script.select(file);
                script.act(KeyCode::Char('s'), "stage");
                script.expect(move |root, _| {
                    assert_eq!(
                        git_index_bytes(root, file),
                        match file {
                            "a-modified" => Some(b"changed\n".to_vec()),
                            "b-new" => Some(b"new\n".to_vec()),
                            "c-deleted" => None,
                            "d-renamed" => Some(b"base d-old\n".to_vec()),
                            "sub/日本語.txt" => Some("変更\n".as_bytes().to_vec()),
                            _ => unreachable!(),
                        },
                        "selected stage target: {file}"
                    )
                });
                script.select(file);
                script.act(KeyCode::Char('u'), "unstage");
                script.expect(move |root, _| {
                    assert_eq!(
                        git_index_bytes(root, file),
                        match file {
                            "b-new" | "d-renamed" => None,
                            _ => Some(format!("base {file}\n").into_bytes()),
                        },
                        "selected unstage target: {file}"
                    )
                });
                run_writes(&roots, "git", &text, script);
            }
        }
        git_all_discard_commit_cases(lang);
        git_branch_write_cases(lang);
    }
    report_write_cases();
}

#[test]
fn confirmation_retry_and_read_only_match_host() {
    if isolated("confirmation_retry_and_read_only_match_host") {
        return;
    }
    write_oracle_negative_controls();
    for lang in ["ja", "en"] {
        submit_recheck_cases(lang);
        for cancel in [KeyCode::Char('n'), KeyCode::Esc] {
            let roots = write_repositories(true);
            let text = config("git", lang, "vim", "relative", "right", "");
            let mut script = WriteScript::default();
            script.select("a-modified");
            script.key(KeyCode::Char('x'));
            script.prompt("confirm", "");
            script.key(KeyCode::Enter);
            script.prompt("confirm", "");
            script.key(cancel);
            script.message(lang, crate::i18n::Msg::Canceled);
            script.expect(|root, state| {
                assert!(state.prompt.is_none());
                assert_eq!(
                    std::fs::read(root.join("a-modified")).unwrap(),
                    b"changed\n"
                );
                assert!(state.notice.is_some());
            });
            script.key(KeyCode::Char('x'));
            script.act(KeyCode::Char('Y'), "discard");
            script.expect(|root, _| {
                assert_eq!(
                    std::fs::read(root.join("a-modified")).unwrap(),
                    b"base a-modified\n"
                )
            });
            run_writes(&roots, "git", &text, script);
        }
        let roots = write_repositories(true);
        let text = config(
            "git",
            lang,
            "vim",
            "relative",
            "right",
            "[plugins.git]\nread_only=true\n[keys.text]\nJ='navigate:down'\n",
        );
        let mut script = WriteScript::default();
        for code in ['s', 'u', 'S', 'U', 'x', 'c'] {
            script.key(KeyCode::Char(code));
            script.message(lang, crate::i18n::Msg::PluginReadOnly);
            script.expect(|_, state| {
                assert!(
                    state.prompt.is_none(),
                    "read-only must refuse before a prompt"
                );
                assert!(state.notice.is_some(), "read-only notice");
            });
        }
        script.key(KeyCode::Char('J'));
        script.key(KeyCode::Enter);
        script.key(KeyCode::Char('q'));
        script.key(KeyCode::Char('b'));
        for code in [KeyCode::Enter, KeyCode::Char('c'), KeyCode::Char('d')] {
            script.key(code);
            script.message(lang, crate::i18n::Msg::PluginReadOnly);
            script.expect(|_, state| assert!(state.prompt.is_none()));
        }
        run_writes(&roots, "git", &text, script);
        let roots = branch_write_repositories();
        let mut script = WriteScript::default();
        script.key(KeyCode::Char('b'));
        script.select("alpha");
        for code in [KeyCode::Enter, KeyCode::Char('c'), KeyCode::Char('d')] {
            script.key(code);
            script.expect(|_, state| {
                assert!(
                    state.prompt.is_none(),
                    "read-only branch action before prompt"
                )
            });
            script.notice(crate::i18n::tr(
                if lang == "ja" {
                    crate::i18n::Lang::Jp
                } else {
                    crate::i18n::Lang::En
                },
                crate::i18n::Msg::PluginReadOnly,
            ));
        }
        run_writes(&roots, "git", &text, script);
    }
    if !crate::vcs::jj::available() {
        eprintln!(
            "NOT RUN: write confirmations: jj unavailable; 0 jj sessions run; 16 jj-only/colocated sync sessions not run"
        );
    }
    if !crate::vcs::jj::available() {
        return;
    }
    // This direct guarded call also lets the optional-binary source audit recognize this test.
    jj_write_cases();
    report_write_cases();
}

fn submit_recheck_cases(lang: &str) {
    let text = config("git", lang, "vim", "relative", "right", "");
    for input in [false, true] {
        let roots = write_repositories(true);
        let mut script = WriteScript::default();
        if input {
            script.act(KeyCode::Char('S'), "stage_all");
            script.key(KeyCode::Char('c'));
            script.text("submit guard");
            script.prompt("input", "submit guard");
        } else {
            script.key(KeyCode::Char('x'));
            script.prompt("confirm", "");
        }
        script
            .steps
            .push(Step::Policy("[plugins.git]\nread_only=true\n".into()));
        script.key(if input {
            KeyCode::Enter
        } else {
            KeyCode::Char('y')
        });
        // Refused before it ran: the input gives the typed text back; a confirmation just closes.
        if input {
            script.prompt("input", "submit guard");
        } else {
            script.expect(|_, state| {
                assert!(state.prompt.is_none(), "a refused confirmation closes the dialog")
            });
        }
        script.notice(crate::i18n::tr(
            if lang == "ja" {
                crate::i18n::Lang::Jp
            } else {
                crate::i18n::Lang::En
            },
            crate::i18n::Msg::PluginReadOnly,
        ));
        run_writes(&roots, "git", &text, script);
    }
}

fn git_all_discard_commit_cases(lang: &str) {
    use super::vcs_changes::write_git_output as git;
    let text = config("git", lang, "vim", "relative", "right", "");
    let roots = write_repositories(false);
    for root in [&roots.host, &roots.app] {
        std::fs::write(root.join(".git/index.lock"), "fixture lock").unwrap();
    }
    let mut script = WriteScript::default();
    for c in ['s', 'u', 'x'] {
        script.key(KeyCode::Char(c));
        script.expect(|_, state| {
            assert!(state.prompt.is_none());
            assert!(state.notice.is_none());
        });
    }
    // The real host dispatches the all-actions on empty repositories; their plugin guards
    // flash without running Git. File actions on the empty placeholder never dispatch.
    script.act(KeyCode::Char('S'), "stage_all");
    script.message(lang, crate::i18n::Msg::NoChanges);
    script.act(KeyCode::Char('U'), "unstage_all");
    script.notice(if lang == "ja" { "ステージ無し" } else { "nothing staged" });
    script.expect(|_, state| assert!(state.prompt.is_none()));
    run_writes(&roots, "git", &text, script);

    let roots = write_repositories(true);
    let mut script = WriteScript::default();
    script.act(KeyCode::Char('S'), "stage_all");
    script.act(KeyCode::Char('U'), "unstage_all");
    script.act(KeyCode::Char('U'), "unstage_all");
    script.select("b-new");
    script.key(KeyCode::Char('x'));
    script.act(KeyCode::Char('y'), "discard");
    script.expect(|_, state| assert!(state.notice.is_some()));
    script.select("a-modified");
    script.key(KeyCode::Char('x'));
    script.act(KeyCode::Char('y'), "discard");
    script.expect(|root, _| {
        assert_eq!(
            std::fs::read(root.join("a-modified")).unwrap(),
            b"base a-modified\n"
        )
    });
    script.act(KeyCode::Char('S'), "stage_all");
    script.key(KeyCode::Char('c'));
    script.prompt("input", "");
    script.text(" ");
    script.act(KeyCode::Enter, "commit");
    script.expect(|_, state| assert!(state.prompt.is_none()));
    script.before(KeyCode::Char('c'), |root| {
        std::fs::create_dir_all(root.join(".git/hooks")).unwrap();
        crate::test_support::write_executable(
            &root.join(".git/hooks/pre-commit"),
            "#!/bin/sh\necho fixture-hook-refused >&2\nexit 1\n",
        );
    });
    script.text("fixture 日本語");
    script.key(KeyCode::Left);
    script.key(KeyCode::Right);
    script.act(KeyCode::Enter, "commit");
    script.prompt("input", "fixture 日本語");
    script.expect(|_, state| assert_eq!(state.notice.as_deref(), Some("fixture-hook-refused")));
    script.before(KeyCode::End, |root| {
        std::fs::remove_file(root.join(".git/hooks/pre-commit")).unwrap()
    });
    script.act(KeyCode::Enter, "commit");
    script.expect(|root, state| {
        assert!(state.prompt.is_none());
        assert_eq!(git(root, &["log", "-1", "--format=%s"]), "fixture 日本語\n");
        assert_eq!(git(root, &["status", "--porcelain=v1"]), "");
    });
    run_writes(&roots, "git", &text, script);
}

fn git_branch_write_cases(lang: &str) {
    let roots = branch_write_repositories();
    let text = config("git", lang, "vim", "relative", "right", "");
    let mut script = WriteScript::default();
    script.key(KeyCode::Char('b'));
    script.act(KeyCode::Char('d'), "delete_current_branch");
    script.expect(|_, state| assert!(state.prompt.is_none()));
    script.act(KeyCode::Enter, "checkout");
    script.select("alpha");
    script.act(KeyCode::Enter, "checkout");
    script.select("main");
    script.act(KeyCode::Enter, "checkout");
    script.key(KeyCode::Char('c'));
    script.text(" ");
    script.act(KeyCode::Enter, "create_branch");
    script.expect(|_, state| assert!(state.prompt.is_none()));
    script.key(KeyCode::Char('c'));
    script.text("  alpha  ");
    script.act(KeyCode::Enter, "create_branch");
    script.prompt("input", "  alpha  ");
    script.key(KeyCode::Esc);
    script.key(KeyCode::Char('c'));
    script.text("  created  ");
    script.act(KeyCode::Enter, "create_branch");
    script.select("alpha");
    script.key(KeyCode::Char('d'));
    script.prompt("confirm", "");
    script.key(KeyCode::Char('n'));
    script.key(KeyCode::Char('d'));
    script.key(KeyCode::Esc);
    script.key(KeyCode::Char('d'));
    script.act(KeyCode::Char('y'), "delete_branch");
    script.select("日本語");
    script.key(KeyCode::Char('d'));
    script.act(KeyCode::Char('y'), "delete_branch");
    script.expect(|_, state| assert!(state.notice.as_ref().unwrap().contains("not fully merged")));
    script.key(KeyCode::Char('d'));
    script.act(KeyCode::Char('!'), "delete_branch");
    run_writes(&roots, "git", &text, script);
}

fn branch_write_repositories() -> WriteRepositories {
    let parent = at_home(sandbox("vcs_app_branch_writes"));
    let roots = WriteRepositories {
        host: parent.join("host/repo"),
        app: parent.join("apps/repo"),
    };
    for root in [&roots.host, &roots.app] {
        std::fs::create_dir_all(root.parent().unwrap()).unwrap();
        let fixture = super::vcs_changes::branch_write_fixture();
        std::fs::rename(&fixture, root).unwrap();
        use super::vcs_changes::write_git_output as git;
        git(root, &["switch", "日本語"]);
        std::fs::write(root.join("unmerged"), "only here\n").unwrap();
        git(root, &["add", "."]);
        git(root, &["commit", "-qm", "unmerged"]);
        git(root, &["switch", "main"]);
    }
    roots
}
