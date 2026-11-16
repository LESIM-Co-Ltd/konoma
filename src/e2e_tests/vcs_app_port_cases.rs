// U9b1 recipes keep the old assertion's complete matrix on both real project paths.

fn compare_ported_frame(expected: &Buffer, actual: &Buffer) -> Result<(), String> {
    // Copy the entire observed rect, including continuation and final-column padding cells.
    let rect = Rect::new(
        actual.area.x,
        actual.area.y,
        actual.area.width,
        actual.area.height,
    );
    let mut captured = Buffer::empty(rect);
    for y in rect.y..rect.bottom() {
        for x in rect.x..rect.right() {
            captured[(x, y)] = actual[(x, y)].clone();
        }
    }
    compare_frames(expected, &captured)
}

fn port_config(id: &str, lang: &str, align: &str) -> String {
    config(
        id,
        lang,
        "vim",
        "relative",
        align,
        "[keys.text]\nD='navigate:down'\n",
    )
}

fn port_list(host: &Sim) -> konoma_plugin::view::ListView {
    let content = host.app.project_content().unwrap();
    let ProjectContent::List(list) = content.as_ref() else {
        panic!("list required")
    };
    list.clone()
}

#[test]
fn vcs_port_unborn_log_cells() {
    if isolated("vcs_port_unborn_log_cells") {
        return;
    }
    let root = at_home(repo(0));
    for lang in ["en", "ja"] {
        let steps = vec![Step::Resize(80, 8), key(KeyCode::Tab)];
        run(
            &root,
            "git",
            &port_config("git", lang, "right"),
            steps,
            false,
            |step, state, host, _| {
                if step >= 2 {
                    assert_eq!(
                        state.stack.last().map(String::as_str),
                        Some(["log", "graph", "branches", "worktrees", "changes"][step - 2])
                    );
                }
                if step == 2 {
                    assert!(state.selected.is_none());
                    let list = port_list(host);
                    assert_eq!(list.rows.len(), 1);
                    assert!(!list.rows[0].selectable);
                    assert_eq!(
                        list.rows[0].cells[0].plain(),
                        if lang == "en" {
                            "  (no commits)"
                        } else {
                            "  (コミットなし)"
                        }
                    );
                }
            },
        );
    }
    report_cases();
}

#[test]
fn vcs_port_jj_changes_fixture_matrix_cells_and_parent_baseline() {
    if !crate::vcs::jj::available() {
        return;
    }
    if isolated("vcs_port_jj_changes_fixture_matrix_cells_and_parent_baseline") {
        return;
    }
    use crate::test_support::{jj_scratch_bare, run_jj};
    for colocated in [false, true] {
        for changed in [false, true] {
            let root = if colocated {
                let root = git_repo("vcs_finish_colocated", false);
                assert!(run_jj(&root, &["git", "init", "--colocate", "."]));
                root
            } else {
                let root = jj_scratch_bare("vcs_finish_jj").expect("available jj fixture");
                for file in [
                    "a-staged.txt",
                    "b-unstaged.txt",
                    "c-deleted.txt",
                    "d-old.txt",
                    "日本語.txt",
                ] {
                    std::fs::write(root.join(file), "base\n").unwrap();
                }
                assert!(run_jj(&root, &["commit", "-m", "seed"]));
                root.into_canonical().unwrap()
            };
            if changed {
                std::fs::write(root.join("a-staged.txt"), "worktree\n").unwrap();
                std::fs::remove_file(root.join("c-deleted.txt")).unwrap();
                std::fs::rename(root.join("d-old.txt"), root.join("d-renamed.txt")).unwrap();
                std::fs::write(root.join("e-untracked.txt"), "new\n").unwrap();
                std::fs::write(root.join("日本語.txt"), "変更\n").unwrap();
            }
            let root = at_home(root);
            let files = backend("jj").changed_files(&root);
            let expected = backend("jj").file_diff(&root, &root.join("a-staged.txt"));
            if changed {
                assert!(expected.iter().any(|l| l.text.contains("base")));
            }
            let mut steps = vec![Step::Resize(140, 20)];
            if changed {
                steps.push(key(KeyCode::Enter));
            }
            run(
                &root,
                "jj",
                &port_config("jj", "en", "right"),
                steps,
                false,
                |step, state, host, _| {
                    if step == 1 {
                        assert_eq!(
                            state.rows.iter().filter(|(_, on)| *on).count(),
                            files.len(),
                            "jj changed row count {colocated}/{changed}"
                        );
                    }
                    if step == 2 && changed {
                        assert!(host.app.is_git_diff_preview());
                        assert_eq!(host.app.tab.preview_path, Some(root.join("a-staged.txt")));
                        let actual = &state
                            .diff
                            .as_ref()
                            .expect("file diff snapshot")
                            .source_lines;
                        assert_eq!(
                            *actual,
                            expected
                                .iter()
                                .map(|l| (
                                    format!("{:?}", l.kind),
                                    l.old_no,
                                    l.new_no,
                                    l.text.clone()
                                ))
                                .collect::<Vec<_>>(),
                            "jj parent baseline {colocated}"
                        );
                    }
                },
            );
        }
    }
    report_cases();
}

#[test]
fn vcs_port_changes_every_selection_empty_and_nested_paths() {
    if isolated("vcs_port_changes_every_selection_empty_and_nested_paths") {
        return;
    }
    for changed in [true, false] {
        let root = at_home(git_repo("vcs_port_changes", changed));
        let count = if changed { 7 } else { 0 };
        assert_eq!(crate::git::changed_files(&root).len(), count);
        for lang in ["en", "ja"] {
            for (width, height) in [(140, 20), (34, 8)] {
                let mut steps = vec![Step::Resize(width, height)];
                steps.extend((1..count).map(|_| ch('j')));
                steps.extend((1..count).map(|_| ch('k')));
                let text = port_config("git", lang, "right");
                run(&root, "git", &text, steps, false, |step, state, _, _| {
                    if step > 0 {
                        assert_eq!(state.rows.iter().filter(|(_, on)| *on).count(), count);
                        let cursor = if step <= count.max(1) {
                            step - 1
                        } else {
                            2 * count - step - 1
                        };
                        assert_eq!(state.cursor, cursor);
                        assert_eq!(state.selected.is_some(), changed);
                    }
                });
            }
        }
    }
    let root = at_home(git_repo("vcs_port_paths", true));
    for launch in [root.as_path()] {
        for style in ["relative", "home", "full"] {
            let text = config("git", "en", "vim", style, "right", "");
            run(
                launch,
                "git",
                &text,
                vec![Step::Resize(200, 20)],
                false,
                |_, state, host, _| {
                    assert_eq!(host.app.tab.projects.projects[0].root, root);
                    assert_eq!(state.rows.len(), 7);
                },
            );
        }
    }
    report_cases();
}

#[test]
fn vcs_port_log_boundary_widths_motions_and_every_query() {
    if isolated("vcs_port_log_boundary_widths_motions_and_every_query") {
        return;
    }
    use unicode_width::UnicodeWidthStr;
    let root = at_home(repo(13));
    let entries = crate::git::log(&root, 200);
    let mw = format!("{} · 2020-01-01 00:12", entries[0].author).width() as u16;
    let motions = [
        KeyCode::Char('j'),
        KeyCode::Char('G'),
        KeyCode::Char('k'),
        KeyCode::Char('g'),
        KeyCode::PageDown,
        KeyCode::PageUp,
        KeyCode::End,
        KeyCode::Home,
        KeyCode::Down,
        KeyCode::Up,
    ];
    for lang in ["en", "ja"] {
        for align in ["right", "inline", "unknown"] {
            for width in [mw + 3, mw + 4, 12, 58, 220] {
                let mut steps = vec![Step::Resize(width, 8), key(KeyCode::Tab)];
                steps.extend(motions.into_iter().map(key));
                let cursors = [0, 1, 12, 11, 0, 10, 0, 12, 0, 1, 0];
                run(
                    &root,
                    "git",
                    &port_config("git", lang, align),
                    steps,
                    false,
                    |step, state, _, _| {
                        if step >= 2 {
                            assert_eq!(
                                state.cursor,
                                cursors[step - 2],
                                "{lang}/{align}/{width}/step {step}"
                            );
                            assert_eq!(state.rows.len(), 13);
                        }
                    },
                );
            }
        }
    }
    for align in ["right", "inline"] {
        for query in [
            "長い件名",
            "作者",
            "Author",
            "2020-01-01 00:12",
            entries[0].short.as_str(),
            "Summary 作者",
            "·",
            "SUMMARY",
            "summary",
        ] {
            let expected: Vec<_> = entries
                .iter()
                .filter(|c| {
                    format!(
                        "{} {} {} {}",
                        c.short,
                        c.summary,
                        c.author,
                        konoma_base::epoch::format_epoch_short(c.time_epoch.max(0) as u64)
                    )
                    .to_lowercase()
                    .contains(&query.to_lowercase())
                })
                .map(|c| c.id.clone())
                .collect();
            let mut steps = vec![Step::Resize(100, 8), key(KeyCode::Tab), ch('/')];
            steps.extend(typed(query));
            steps.push(key(KeyCode::Enter));
            let applied = steps.len();
            steps.push(ch('j'));
            run(
                &root,
                "git",
                &port_config("git", "en", align),
                steps,
                false,
                |step, state, _, _| {
                    if step >= applied {
                        assert_eq!(
                            state
                                .visible
                                .iter()
                                .map(|r| r.value.clone())
                                .collect::<Vec<_>>(),
                            expected,
                            "query={query}"
                        );
                        assert_eq!(
                            state.cursor,
                            if step == applied || expected.is_empty() {
                                0
                            } else {
                                1.min(expected.len() - 1)
                            }
                        );
                    }
                },
            );
        }
    }
    report_cases();
}

#[test]
fn vcs_port_detail_full_matrix_and_cursor_return() {
    if isolated("vcs_port_detail_full_matrix_and_cursor_return") {
        return;
    }
    let root = at_home(detail_repo());
    for width in [80, 180] {
        for layout in ["unified", "auto"] {
            for empty in [false, true] {
                for scheme in ["vim", "less"] {
                    let text = config("git", "en", scheme, "relative", "right", "");
                    let text = text.replace("diff='unified'", &format!("diff='{layout}'"));
                    let mut steps = vec![Step::Resize(width, 14), key(KeyCode::Tab)];
                    if !empty {
                        steps.push(ch('j'));
                    }
                    steps.push(key(KeyCode::Enter));
                    let opened = steps.len();
                    for (code, mods) in [
                        (KeyCode::Char('j'), KeyModifiers::NONE),
                        (KeyCode::Char('j'), KeyModifiers::NONE),
                        (KeyCode::Char('j'), KeyModifiers::NONE),
                        (KeyCode::Char('d'), KeyModifiers::CONTROL),
                        (KeyCode::Char('G'), KeyModifiers::NONE),
                        (KeyCode::Char('l'), KeyModifiers::NONE),
                        (KeyCode::Char('l'), KeyModifiers::NONE),
                        (KeyCode::Char('s'), KeyModifiers::NONE),
                        (KeyCode::Char('l'), KeyModifiers::NONE),
                        (KeyCode::Char('0'), KeyModifiers::NONE),
                        (KeyCode::Char('$'), KeyModifiers::NONE),
                        (KeyCode::Char('h'), KeyModifiers::NONE),
                        (KeyCode::Char('g'), KeyModifiers::NONE),
                        (KeyCode::PageDown, KeyModifiers::NONE),
                        (KeyCode::PageUp, KeyModifiers::NONE),
                    ] {
                        steps.push(Step::Key(KeyEvent::new(code, mods)));
                    }
                    for c in ['d', 'u', 'f', 'b'] {
                        steps.push(Step::Key(KeyEvent::new(
                            KeyCode::Char(c),
                            if scheme == "less" {
                                KeyModifiers::NONE
                            } else {
                                KeyModifiers::CONTROL
                            },
                        )));
                    }
                    let cycled = steps.len();
                    steps.extend([
                        key(KeyCode::Tab),
                        key(KeyCode::BackTab),
                        ch('q'),
                        ch('l'),
                        key(KeyCode::Esc),
                    ]);
                    run(&root, "git", &text, steps, false, |step, state, host, _| {
                        if step >= opened && ![cycled + 3, cycled + 5].contains(&step) {
                            assert_eq!(state.stack.last().unwrap(), "detail");
                            let content = host.app.project_content().unwrap();
                            let ProjectContent::Diff(diff) = content.as_ref() else {
                                panic!("diff required")
                            };
                            assert_eq!(diff.lines.is_empty(), empty);
                            if !empty {
                                assert_eq!(diff.lines.len(), 142);
                            }
                            if step == opened + 8 {
                                assert_eq!(state.hscroll, 0);
                            }
                        }
                        if [cycled + 3, cycled + 5].contains(&step) {
                            assert_eq!(state.stack.last().unwrap(), "log");
                            assert_eq!(state.cursor, usize::from(!empty));
                        }
                    });
                }
            }
        }
    }
    report_cases();
}

#[test]
fn vcs_port_missing_detail_refresh_has_no_header_or_lines() {
    if isolated("vcs_port_missing_detail_refresh_has_no_header_or_lines") {
        return;
    }
    for width in [80, 180] {
        for layout in ["unified", "auto"] {
            let root = at_home(detail_repo());
            let text = port_config("git", "en", "right")
                .replace("diff='unified'", &format!("diff='{layout}'"));
            let steps = vec![
                Step::Resize(width, 12),
                key(KeyCode::Tab),
                key(KeyCode::Enter),
                Step::BeforeKey(
                    Box::new(remove_head_object),
                    KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE),
                ),
                ch('j'),
                ch('j'),
                ch('j'),
                Step::Key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL)),
                ch('G'),
                ch('l'),
                ch('l'),
                ch('s'),
                ch('l'),
                ch('0'),
            ];
            let mut before = None;
            run(&root, "git", &text, steps, false, |step, state, host, _| {
                if step == 3 {
                    before = host.app.project_content();
                }
                if step >= 4 {
                    let after = host.app.project_content().unwrap();
                    assert!(!Arc::ptr_eq(before.as_ref().unwrap(), &after));
                    let ProjectContent::Diff(diff) = after.as_ref() else {
                        panic!("diff required")
                    };
                    assert!(diff.header.is_empty() && diff.lines.is_empty());
                    assert_eq!(state.stack.last().unwrap(), "detail");
                }
            });
        }
    }
    report_cases();
}

#[test]
fn vcs_port_picker_all_keys_icons_languages_and_branch_refresh() {
    if isolated("vcs_port_picker_all_keys_icons_languages_and_branch_refresh") {
        return;
    }
    for icons in [false, true] {
        let root = at_home(super::vcs_log::graph_picker_fixture());
        for lang in ["en", "ja"] {
            let text = port_config("git", lang, "right")
                .replace("[ui]\n", &format!("[ui]\nicons={icons}\n"));
            let text = format!("{text}[plugins.git]\ngraph_base_branches=['missing-branch','priority-branch-with-a-long-name']\n");
            let mut steps = vec![Step::Resize(160, 16), key(KeyCode::Tab), key(KeyCode::Tab)];
            steps.extend("bxjx acJKGkg".chars().filter(|c| *c != ' ').map(ch));
            steps.extend([
                Step::Key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL)),
                Step::Key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL)),
                key(KeyCode::PageDown),
                key(KeyCode::Home),
                key(KeyCode::Left),
                key(KeyCode::Right),
            ]);
            run(&root, "git", &text, steps, false, |_, state, host, _| {
                if state.stack.last().map(String::as_str) == Some("graph_picker") {
                    assert_eq!(state.stack.len(), 2);
                    let graph = state.graph.as_ref().unwrap();
                    let title = port_list(host).title.unwrap().plain();
                    let shown = graph.tentative.len();
                    let count = graph.order.len();
                    assert_eq!(
                        title,
                        format!(
                            " {}  ({shown}/{count}) ",
                            crate::i18n::tr(host.app.lang, crate::i18n::Msg::GraphPickerTitle)
                        )
                    );
                    assert!(!state.filtering);
                    assert_eq!(count, 4);
                }
            });
        }
        let text =
            port_config("git", "en", "right").replace("[ui]\n", &format!("[ui]\nicons={icons}\n"));
        let steps = vec![
            key(KeyCode::Tab),
            key(KeyCode::Tab),
            ch('b'),
            ch('q'),
            Step::BeforeKey(
                Box::new(|p| {
                    let repository = konoma_vcs_core::git2::Repository::open(p).unwrap();
                    let head = repository.head().unwrap().peel_to_commit().unwrap();
                    repository.branch("added-branch", &head, false).unwrap();
                }),
                KeyEvent::new(KeyCode::Char('b'), KeyModifiers::NONE),
            ),
            ch('/'),
            ch('b'),
        ];
        let mut before_filter = None;
        run(&root, "git", &text, steps, false, |step, state, host, _| {
            if step == 3 {
                assert_eq!(state.rows.len(), 4);
            }
            if step == 5 {
                assert_eq!(
                    state.rows.len(),
                    5,
                    "opening the picker rereads git branches"
                );
                assert!(state.rows.iter().any(|(r, _)| r.value == "added-branch"));
                before_filter = Some(port_list(host));
            }
            if step >= 6 {
                assert_eq!(
                    state.stack.last().unwrap(),
                    "graph_picker",
                    "shared jumps do not replace a pushed picker"
                );
                assert!(!state.filtering);
                assert_eq!(port_list(host), before_filter.clone().unwrap());
                assert!(
                    !crate::ops::footer::tokens(&host.app, crate::keymap::Context::Canvas)
                        .iter()
                        .any(|s| s.starts_with("/:"))
                );
            }
        });
    }
    for screen in ["changes", "log"] {
        let root = at_home(super::vcs_log::graph_picker_fixture());
        let mut steps = vec![];
        if screen == "log" {
            steps.push(key(KeyCode::Tab));
        }
        steps.push(ch('b'));
        let end = steps.len();
        run_roots_with(
            (&root, &root),
            "git",
            &port_config("git", "en", "right"),
            steps,
            false,
            port_branch_options(),
            |step, state, _, _| {
                if step == end {
                    assert_eq!(state.stack, ["branches"]);
                }
            },
        );
    }
    report_cases();
}

#[test]
fn vcs_port_picker_apply_and_every_cancel_key() {
    if isolated("vcs_port_picker_apply_and_every_cancel_key") {
        return;
    }
    let root = at_home(super::vcs_log::graph_picker_fixture());
    let text = format!("{}[plugins.git]\ngraph_base_branches=['missing-branch','priority-branch-with-a-long-name']\n", port_config("git", "en", "right"));
    for apply in [KeyCode::Char('l'), KeyCode::Enter] {
        for reorder in [false, true] {
            let mut steps = vec![
                Step::Resize(160, 16),
                key(KeyCode::Tab),
                key(KeyCode::Tab),
                ch('j'),
                ch('b'),
                ch('x'),
            ];
            if reorder {
                steps.push(ch('J'));
            }
            steps.push(key(apply));
            let applied = steps.len();
            steps.push(ch('b'));
            let mut visible = None;
            let mut order = None;
            run(&root, "git", &text, steps, false, |step, state, host, _| {
                if step == applied - 1 {
                    visible = Some(state.graph.as_ref().unwrap().tentative.clone());
                    order = Some(state.graph.as_ref().unwrap().order.clone());
                }
                if step == applied {
                    assert_eq!(state.stack, ["graph"]);
                    let graph = state.graph.as_ref().unwrap();
                    assert_eq!(Some(graph.visible.clone()), visible);
                    assert_eq!(Some(graph.order.clone()), order);
                    if reorder {
                        let first = graph
                            .order
                            .iter()
                            .find(|n| graph.visible.contains(n))
                            .unwrap();
                        assert_eq!(graph.label.as_ref(), Some(first));
                        assert_eq!(graph.base, crate::git::branch_tip(&root, first));
                    }
                    let title = crate::e2e_tests::vcs_changes::heading_text(&port_list(host));
                    if let Some(label) = &graph.label {
                        assert!(title.contains(&format!("base: {label}")));
                    } else {
                        assert!(!title.contains("base:"));
                    }
                }
                if step == applied + 1 {
                    assert_eq!(state.stack.last().unwrap(), "graph_picker");
                    assert_eq!(
                        Some(state.graph.as_ref().unwrap().tentative.clone()),
                        visible
                    );
                }
            });
        }
    }
    for cancel in [KeyCode::Char('h'), KeyCode::Char('q'), KeyCode::Esc] {
        let steps = vec![
            Step::Resize(160, 16),
            key(KeyCode::Tab),
            key(KeyCode::Tab),
            ch('b'),
            ch('x'),
            key(cancel),
            ch('b'),
            ch('J'),
            key(cancel),
            ch('b'),
        ];
        let mut original = None;
        let mut order = None;
        run(&root, "git", &text, steps, false, |step, state, _, _| {
            if step == 3 {
                original = Some(state.graph.as_ref().unwrap().visible.clone());
            }
            if step == 8 {
                order = Some(state.graph.as_ref().unwrap().order.clone());
            }
            if [6, 9].contains(&step) {
                assert_eq!(state.stack, ["graph"]);
                assert_eq!(
                    Some(state.graph.as_ref().unwrap().visible.clone()),
                    original
                );
            }
            if step == 10 {
                let graph = state.graph.as_ref().unwrap();
                assert_eq!(Some(graph.order.clone()), order);
                assert_eq!(Some(graph.tentative.clone()), original);
            }
        });
    }
    report_cases();
}

fn port_branch_options() -> RunOptions {
    // The write fixture also records the branch screen's explicit root-return difference.
    RunOptions {
        port_branches: true,
        ..Default::default()
    }
}

#[test]
fn vcs_port_graph_widths_queries_connector_motion_and_short_heights() {
    if isolated("vcs_port_graph_widths_queries_connector_motion_and_short_heights") {
        return;
    }
    let root = at_home(super::vcs_log::graph_picker_fixture());
    crate::test_support::run_git(&root, &["branch", "-D", "日本語-branch"]);
    for lang in ["en", "ja"] {
        for width in [80, 160] {
            for (align, cap, priority, icons) in [
                ("right", 12, false, true),
                ("inline", 12, false, true),
                ("right", 1, false, false),
                ("right", 0, true, false),
            ] {
                let mut text = port_config("git", lang, align)
                    .replace("[ui]\n", &format!("[ui]\nicons={icons}\n"));
                text.push_str(&format!(
                    "[plugins.git]\ngraph_max_branches={cap}\ngraph_base_branches={}\n",
                    if priority {
                        "['missing-branch','priority-branch-with-a-long-name']"
                    } else {
                        "[]"
                    }
                ));
                let mut steps = vec![
                    Step::Resize(width, 16),
                    key(KeyCode::Tab),
                    key(KeyCode::Tab),
                    ch('j'),
                    ch('j'),
                ];
                for query in ["日本語", "Main", "nothing-matches", "│"] {
                    steps.push(ch('/'));
                    steps.extend(typed(query));
                    steps.push(key(KeyCode::Enter));
                }
                run(&root, "git", &text, steps, false, |step, state, _, _| {
                    if step == 3 {
                        assert!(state.rows.iter().any(|(r, _)| r.kind == "connector"));
                        let graph = state.graph.as_ref().unwrap();
                        assert_eq!(graph.visible.len(), if cap == 0 { 3 } else { cap.min(3) });
                        if priority {
                            assert_eq!(
                                graph.label.as_deref(),
                                Some("priority-branch-with-a-long-name")
                            );
                        }
                    }
                    if step == 5 {
                        assert!(state.cursor > 2, "two moves cross a connector");
                    }
                    if !state.query.is_empty() && !state.filtering {
                        assert!(state.visible.iter().all(|r| r.kind != "connector"));
                    }
                });
            }
        }
    }
    for width in [80, 160] {
        for height in [5, 6, 7, 8] {
            run(
                &root,
                "git",
                &port_config("git", "en", "right"),
                vec![
                    Step::Resize(width, height),
                    key(KeyCode::Tab),
                    key(KeyCode::Tab),
                    ch('G'),
                    ch('g'),
                ],
                false,
                |_, _, _, _| {},
            );
        }
    }
    report_cases();
}

#[test]
fn vcs_port_jj_and_colocated_log_cells_metadata_and_picker_source() {
    if !crate::vcs::jj::available() {
        return;
    }
    if isolated("vcs_port_jj_and_colocated_log_cells_metadata_and_picker_source") {
        return;
    }
    for colocated in [false, true] {
        let root = at_home(jj_repo(colocated));
        let commits = konoma_vcs_core::vcs::jj::log(&root, 200);
        assert!(commits.iter().any(|c| c.summary == "jj 専用 Summary"));
        assert!(commits
            .iter()
            .filter(|c| c.summary.contains("Summary") && !c.summary.contains("日本語の長い"))
            .any(|c| c.time_epoch == 1_577_838_000));
        for id in if colocated {
            vec!["git", "jj"]
        } else {
            vec!["jj"]
        } {
            let expected = backend(id)
                .log(&root, 200)
                .iter()
                .map(|r| r.id.clone())
                .collect::<Vec<_>>();
            for align in ["right", "inline", "unknown"] {
                for lang in ["en", "ja"] {
                    let mut steps = vec![Step::Resize(72, 7), key(KeyCode::Tab)];
                    steps.extend((0..expected.len()).map(|_| ch('j')));
                    run(
                        &root,
                        id,
                        &port_config(id, lang, align),
                        steps,
                        false,
                        |step, state, _, _| {
                            if step >= 2 {
                                assert_eq!(
                                    state
                                        .visible
                                        .iter()
                                        .map(|r| r.value.clone())
                                        .collect::<Vec<_>>(),
                                    expected
                                );
                            }
                        },
                    );
                }
            }
            if id == "jj" {
                run(
                    &root,
                    id,
                    &port_config(id, "en", "right"),
                    vec![Step::Resize(80, 12), key(KeyCode::Tab), key(KeyCode::Enter)],
                    false,
                    |step, _, host, _| {
                        if step == 3 {
                            let content = host.app.project_content().unwrap();
                            let ProjectContent::Diff(diff) = content.as_ref() else {
                                panic!("diff required")
                            };
                            assert!(diff.header[0].spans[0].content.starts_with("change "));
                        }
                    },
                );
            }
        }
        let count = crate::git::branches_by_recency(&root).len();
        assert_eq!(count == 0, !colocated);
        for lang in ["en", "ja"] {
            let steps = vec![
                Step::Resize(160, 16),
                key(KeyCode::Tab),
                key(KeyCode::Tab),
                ch('b'),
                ch('/'),
                key(KeyCode::Enter),
            ];
            run(
                &root,
                "jj",
                &port_config("jj", lang, "right"),
                steps,
                false,
                |step, state, host, _| {
                    if [4, 5].contains(&step) {
                        assert_eq!(state.stack.last().unwrap(), "graph_picker");
                        assert!(!state.filtering);
                        assert_eq!(state.graph.as_ref().unwrap().order.len(), count);
                        let list = port_list(host);
                        assert_eq!(
                            list.title.unwrap().plain(),
                            format!(
                                " {}  ({count}/{count}) ",
                                crate::i18n::tr(host.app.lang, crate::i18n::Msg::GraphPickerTitle)
                            )
                        );
                    }
                    if step == 6 {
                        assert_eq!(state.stack, ["graph"]);
                    }
                },
            );
        }
    }
    report_cases();
}

#[test]
fn vcs_port_jj_bookmark_cells_copy_and_full_tab_cycle() {
    if !crate::vcs::jj::available() {
        return;
    }
    if isolated("vcs_port_jj_bookmark_cells_copy_and_full_tab_cycle") {
        return;
    }
    let root = at_home(sandbox("vcs_port_bookmarks"));
    jj(&root, &["git", "init", "--no-colocate", "."]);
    jj(&root, &["describe", "-m", "seed"]);
    super::vcs_changes::add_jj_bookmarks(&root);
    for lang in ["en", "ja"] {
        let steps = vec![
            Step::Resize(160, 20),
            ch('b'),
            ch('y'),
            ch('n'),
            key(KeyCode::Enter),
            ch('c'),
            ch('d'),
            key(KeyCode::Tab),
            key(KeyCode::Tab),
            key(KeyCode::Tab),
            key(KeyCode::Tab),
            key(KeyCode::BackTab),
            key(KeyCode::BackTab),
            key(KeyCode::BackTab),
            key(KeyCode::BackTab),
        ];
        run(
            &root,
            "jj",
            &port_config("jj", lang, "right"),
            steps,
            false,
            |step, state, host, writes| {
                if step == 2 {
                    assert!(state.visible.iter().all(|r| r.kind == "branch"));
                    assert_eq!(
                        state
                            .visible
                            .iter()
                            .map(|r| r.value.as_str())
                            .collect::<Vec<_>>(),
                        ["alpha", "日本語"]
                    );
                    let help =
                        crate::ops::help::sections_for(&host.app, crate::keymap::Context::Canvas);
                    assert!(help
                        .iter()
                        .filter(|s| s.title == "jj")
                        .flat_map(|s| &s.rows)
                        .next()
                        .is_none());
                    let copy = crate::ops::menu::menu(
                        &host.app,
                        crate::keymap::Context::Canvas,
                        crate::keymap::LeaderId::Copy,
                        crate::ops::menu::View::Open,
                    )
                    .unwrap();
                    assert_eq!(copy.items.len(), 1);
                }
                if step >= 4 {
                    assert_eq!(writes, ["alpha"]);
                }
                if [5, 6, 7].contains(&step) {
                    assert!(state.prompt.is_none());
                }
                if step == 7 {
                    assert_eq!(konoma_vcs_core::vcs::jj::bookmarks(&root).len(), 2);
                }
                if step >= 8 {
                    let expected = [
                        "changes", "log", "graph", "branches", "graph", "log", "changes",
                        "branches",
                    ];
                    assert_eq!(state.stack, [expected[step - 8]]);
                }
                if state.stack.last().map(String::as_str) == Some("branches") {
                    let hints =
                        crate::ops::footer::tokens(&host.app, crate::keymap::Context::Canvas);
                    assert!(!hints.iter().any(|t| t.starts_with("c:")
                        || t.starts_with("d:")
                        || t.starts_with("Enter:")));
                }
            },
        );
    }
    report_cases();
}

#[test]
fn vcs_port_graph_every_copy_base_reload_and_both_detail_open_keys() {
    if isolated("vcs_port_graph_every_copy_base_reload_and_both_detail_open_keys") {
        return;
    }
    let root = at_home(super::vcs_log::graph_picker_fixture());
    crate::test_support::run_git(&root, &["branch", "-D", "日本語-branch"]);
    for lang in ["en", "ja"] {
        let mut steps = vec![
            Step::Resize(160, 16),
            key(KeyCode::Tab),
            key(KeyCode::Tab),
            ch('s'),
            ch('y'),
            key(KeyCode::Enter),
            ch('q'),
            ch('l'),
            ch('q'),
            ch('j'),
        ];
        for c in ['s', 'h', 't', 'm', 'a', 'd'] {
            steps.extend([ch('y'), ch(c)]);
        }
        let graph = backend("git").graph(
            false,
            &root,
            None,
            &konoma_vcs_core::git::WorktreeLabels::default(),
            None,
        );
        let side = graph
            .iter()
            .find(|r| r.subject == "Side 日本語")
            .unwrap()
            .commit
            .clone()
            .unwrap();
        let moves = graph
            .iter()
            .filter(|r| r.commit.is_some() || r.worktree)
            .position(|r| r.commit.as_ref() == Some(&side))
            .unwrap()
            - 1;
        steps.extend((0..moves).map(|_| ch('j')));
        steps.extend([
            ch('s'),
            ch('0'),
            ch('x'),
            ch('x'),
            ch('r'),
            key(KeyCode::Enter),
            ch('q'),
            ch('l'),
            ch('q'),
        ]);
        let pinned = steps.len() - 8;
        let expected = backend("git")
            .log(&root, 200)
            .into_iter()
            .find(|r| r.summary == "Merge 日本語")
            .unwrap();
        let expected = copies(&backend("git").commit_meta(&root, &expected.id).unwrap());
        assert!(expected[3].contains("\n\nBody"));
        let mut selected = None;
        run(
            &root,
            "git",
            &port_config("git", lang, "right"),
            steps,
            false,
            |step, state, host, writes| {
                if step == 4 {
                    assert_eq!(
                        state.notice.as_deref(),
                        Some(match host.app.lang {
                            crate::i18n::Lang::En => "select a commit to set as base",
                            crate::i18n::Lang::Jp => "基準にするコミットを選んでください",
                        })
                    );
                }
                if step == 5 {
                    assert_eq!(state.menu, None);
                }
                if [6, 8].contains(&step) {
                    assert_eq!(state.stack.last().unwrap(), "detail");
                }
                if [7, 9].contains(&step) {
                    assert_eq!(state.stack, ["graph"]);
                }
                assert_eq!(writes, &expected[..writes.len()]);
                if step == 22 {
                    assert_eq!(writes.len(), 6);
                }
                if step == pinned {
                    selected = state.selected.clone();
                    assert_eq!(state.selected.as_ref().unwrap().value, side);
                    assert_eq!(
                        state.graph.as_ref().unwrap().base.as_deref(),
                        state.selected.as_ref().map(|r| r.value.as_str())
                    );
                }
                if step == pinned + 1 {
                    assert!(
                        state.graph.as_ref().unwrap().base.is_some(),
                        "0 does not clear a base"
                    );
                }
                if [pinned + 2, pinned + 3].contains(&step) {
                    assert!(state.graph.as_ref().unwrap().base.is_none());
                }
                if step == pinned + 3 {
                    assert!(state.notice.is_none(), "clearing an unset base is silent");
                }
                if step >= pinned && state.stack.last().map(String::as_str) == Some("graph") {
                    assert_eq!(state.selected, selected);
                }
            },
        );
    }
    report_cases();
}

#[test]
fn vcs_port_log_205_commits_keep_exactly_the_first_200() {
    if isolated("vcs_port_log_205_commits_keep_exactly_the_first_200") {
        return;
    }
    let root = at_home(repo(205));
    assert_eq!(backend("git").log(&root, 206).len(), 205);
    let expected: Vec<_> = backend("git")
        .log(&root, 200)
        .iter()
        .map(|r| r.id.clone())
        .collect();
    run(
        &root,
        "git",
        &port_config("git", "en", "right"),
        vec![Step::Resize(100, 8), key(KeyCode::Tab), ch('G')],
        false,
        |step, state, _, _| {
            if step >= 2 {
                assert_eq!(state.visible.len(), 200);
                assert_eq!(
                    state
                        .visible
                        .iter()
                        .map(|r| r.value.clone())
                        .collect::<Vec<_>>(),
                    expected
                );
            }
            if step == 3 {
                assert_eq!(state.cursor, 199);
            }
        },
    );
    report_cases();
}
