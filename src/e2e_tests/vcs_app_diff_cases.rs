// OpenDiff uses the same uninterrupted host/standalone input driver as the project cases.

const CONTRACT_ROOTS: [&str; 5] = ["one", "two", "three", "four", "five"];

type HelpSections = Vec<(String, Vec<(String, String)>)>;
fn expected_preview_help_sections(
    sections: &HelpSections,
    lang: &str,
    document: bool,
) -> HelpSections {
    let fixture = read_snapshot_for_test("vcs_app_preview_help");
    sections
        .iter()
        .filter_map(|(title, rows)| {
            let rows: Vec<_> = rows
                .iter()
                .filter_map(|row| {
                    let allowed = fixture[lang].as_array().unwrap().iter().find(|rule| {
                        (document || !matches!(rule["design"].as_u64(), Some(447 | 474)))
                            && rule["host"][0].as_str() == Some(row.0.as_str())
                            && rule["host"][1].as_str() == Some(row.1.as_str())
                    });
                    match allowed {
                        None => Some(row.clone()),
                        Some(rule) => {
                            match rule["design"].as_u64().unwrap() {
                                447 => {}  // Design :447: document search and outline are host-only.
                                474 => {} // Design :474: Tab focus, focused links/tasks/details/diagrams are host-only.
                                476 => {} // Design :476: Y reference copy is host-only.
                                1266 => {} // Design :1266: tabs, follow, P, bookmarks and Ctrl-n/Ctrl-p file jump are absent; konoma-vcs keeps p/!.
                                line => panic!(
                                    "unapproved Preview help difference at design line {line}"
                                ),
                            }
                            (!rule["app"].is_null()).then(|| {
                                (
                                    rule["app"][0].as_str().unwrap().into(),
                                    rule["app"][1].as_str().unwrap().into(),
                                )
                            })
                        }
                    }
                })
                .collect();
            (!rows.is_empty()).then(|| (title.clone(), rows))
        })
        .collect()
}

fn compare_preview_help(
    expected: &mut Buffer,
    host: &Buffer,
    app: &Buffer,
    host_help: &konoma_plugin::app::project::ProjectHelpSnapshot,
    app_help: &konoma_plugin::app::project::ProjectHelpSnapshot,
    lang: &str,
    document: bool,
) {
    let wanted_sections = expected_preview_help_sections(&host_help.sections, lang, document);
    if app_help.sections != wanted_sections {
        dump_parity_frame("host help rows differ", host);
        dump_parity_frame("app help rows differ", app);
    }
    assert_eq!(
        app_help.sections, wanted_sections,
        "every Preview help row, including off-screen rows; only the explicit design rows may differ"
    );
    assert_eq!(app_help.title, host_help.title);
    for (side, frame, help) in [("host", host, host_help), ("app", app, app_help)] {
        let refs: Vec<_> = help
            .sections
            .iter()
            .map(|(title, rows)| (title.as_str(), rows.as_slice()))
            .collect();
        let lines = konoma_widgets::help_panel::help_lines(&refs);
        let (popup, _) = konoma_widgets::help_panel::popup_layout(lines.len(), frame.area);
        let mut wanted = Buffer::empty(frame.area);
        konoma_widgets::help_panel::render_panel(
            &mut wanted,
            frame.area,
            lines,
            help.scroll as u16,
            help.title.clone(),
        );
        for y in popup.y..popup.bottom() {
            for x in popup.x..popup.right() {
                assert_eq!(
                    frame[(x, y)],
                    wanted[(x, y)],
                    "{side} Preview help cell ({x},{y}) at scroll {}",
                    help.scroll
                );
                if side == "app" {
                    expected[(x, y)] = wanted[(x, y)].clone();
                }
            }
        }
    }
}

#[test]
fn preview_help_comparison_rejects_copied_wrong_row() {
    use konoma_host_core::project::DiffView;
    use konoma_plugin::app::project::{ProjectDiffSnapshot, ProjectHelpSnapshot};
    let help = ProjectHelpSnapshot {
        sections: vec![(
            "Launch".into(),
            vec![("e".into(), "edit in external editor".into())],
        )],
        scroll: 0,
        title: "Help".into(),
    };
    let diff = ProjectDiffSnapshot {
        help: Some(help.clone()),
        path: "doc.md".into(),
        baseline: "HEAD".into(),
        choice: DiffView::Preview,
        effective: DiffView::Preview,
        next_representation: None,
        layout: "Unified".into(),
        resolved_split: false,
        split_drawn: None,
        wrap: true,
        scroll: 0,
        hscroll: 0,
        first_change: None,
        editor_line: None,
        return_to: vec!["changes".into()],
        source_lines: vec![],
        baseline_text: None,
        preview_context: Some("Document".into()),
    };
    let state = ProjectStateSnapshot {
        diff: Some(diff),
        ..Default::default()
    };
    let mut host = Buffer::empty(Rect::new(0, 0, 90, 14));
    let refs: Vec<_> = help
        .sections
        .iter()
        .map(|(t, r)| (t.as_str(), r.as_slice()))
        .collect();
    let lines = konoma_widgets::help_panel::help_lines(&refs);
    let (popup, _) = konoma_widgets::help_panel::popup_layout(lines.len(), host.area);
    konoma_widgets::help_panel::render_panel(
        &mut host,
        Rect::new(0, 0, 90, 14),
        lines,
        0,
        help.title,
    );
    let app = host.clone();
    let fixture = read_snapshot_for_test("vcs_app_chrome");
    let graph = read_snapshot_for_test("vcs_app_graph");
    frames(&host, &app, (&state, &state), "en", &fixture, &graph, true);
    let mut corrupt = app.clone();
    corrupt[(popup.x + 19, popup.y + 2)].set_symbol("X");
    let rejected = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        frames(
            &host,
            &corrupt,
            (&state, &state),
            "en",
            &fixture,
            &graph,
            true,
        )
    }));
    assert!(
        rejected.is_err(),
        "a copied app popup must never hide a differing help row"
    );
}

#[test]
fn project_diff_preview_help_compares_every_page() {
    if isolated("project_diff_preview_help_compares_every_page") {
        return;
    }
    for lang in ["en", "ja"] {
        for width in [48, 180] {
            for path in ["doc.md", "image.png"] {
                let roots = diff_repositories(|| preview_shape_repo(path));
                let text = config(
                    "git",
                    lang,
                    "vim",
                    "relative",
                    "right",
                    "[plugins.git]\nread_only=true\n",
                );
                let mut steps = vec![Step::Resize(width, 14), ch('/')];
                steps.extend(typed(path));
                steps.extend([key(KeyCode::Enter), key(KeyCode::Enter), ch('R')]);
                if path == "doc.md" {
                    steps.push(ch('R'));
                }
                steps.push(ch('?'));
                for _ in 0..100 {
                    steps.push(ch('j'));
                }
                steps.extend([ch('G'), ch('g'), ch('q'), ch('q')]);
                let mut seen = [
                    std::collections::BTreeSet::new(),
                    std::collections::BTreeSet::new(),
                ];
                let mut counts = [0, 0];
                let mut open = false;
                run_roots_with(
                    (&roots.host, &roots.app),
                    "git",
                    &text,
                    steps,
                    false,
                    RunOptions {
                        pictures: true,
                        launches: Some(Arc::new(Mutex::new(Vec::new()))),
                        ..Default::default()
                    },
                    |_, state, host, _| {
                        if let Some(help) = state.diff.as_ref().and_then(|d| d.help.as_ref()) {
                            open = true;
                            let refs: Vec<_> = help
                                .sections
                                .iter()
                                .map(|(t, r)| (t.as_str(), r.as_slice()))
                                .collect();
                            let app_lines = konoma_widgets::help_panel::help_lines(&refs);
                            let host_lines = crate::ui::help::help_lines(&host.app);
                            for (i, len, scroll) in [
                                (0, host_lines.len(), usize::from(host.app.help_scroll)),
                                (1, app_lines.len(), help.scroll),
                            ] {
                                counts[i] = len;
                                let area = Rect::new(0, 0, width, 14);
                                let (_, rows) = konoma_widgets::help_panel::popup_layout(len, area);
                                let top = scroll.min(usize::from(
                                    konoma_widgets::help_panel::max_scroll(len, area),
                                ));
                                seen[i].extend(top..(top + usize::from(rows)).min(len));
                            }
                        } else if open {
                            for i in 0..2 {
                                assert_eq!(seen[i].len(),counts[i],"every rendered help line was visited: side {i}, {lang}, {width}, {path}");
                            }
                            open = false;
                        }
                    },
                );
            }
        }
    }
}

#[test]
fn project_diff_preview_project_key_uses_project_footer_host() {
    if isolated("project_diff_preview_project_key_uses_project_footer_host") {
        return;
    }
    for lang in ["en", "ja"] {
        for width in [48, 180] {
            let root = at_home(preview_shape_repo("doc.md"));
            let cfg: Config = toml::from_str(&config(
                "git",
                lang,
                "vim",
                "relative",
                "right",
                "[plugins.git]\nread_only=true\n",
            ))
            .unwrap();
            let mut host = Sim::with_config_sized(&root, cfg, width, 14).with_media();
            host.app.tab.projects.wait_for_result();
            host.app.poll_projects();
            host.key('p');
            drain(&mut host);
            host.enter();
            drain(&mut host);
            host.key('R');
            host.key('R');
            host.settle_inline_images(
                "Preview before project overlay",
                std::time::Duration::from_secs(30),
            );
            assert!(host.app.preview_from_diff_for_test());
            assert!(!host.app.project_active());
            host.key('p');
            drain(&mut host);
            assert!(host.app.project_active());
            // Old hub: Context::Screen(backend, Changes) after `p`; recorded as pending.
            super::vcs_port_map::pending_behavior(
                "key-context-screen",
                "old key_context() == Some(Context::Screen(backend, Changes)) after p",
            );
            assert_eq!(
                crate::ops::help::help_context(&host.app),
                crate::keymap::Context::Canvas
            );
            let first = host.project_frame();
            let second = host.project_frame();
            compare_frames(&first, &second).expect("p reaches a steady logical frame");
            let actual = crate::ops::footer::tokens(&host.app, crate::keymap::Context::Canvas);
            let project = crate::ops::footer::render(
                &host.app,
                crate::keymap::Context::Canvas,
                &crate::ops::footer::plan(&host.app, crate::keymap::Context::Canvas),
            );
            assert_eq!(actual, project, "a project overlay must use its project footer, not the retained diff Preview's Canvas footer");
            let rows = |s: Vec<crate::ui::help::HelpSection>| {
                s.into_iter().map(|s| (s.title, s.rows)).collect::<Vec<_>>()
            };
            let before = rows(crate::ops::help::sections_for(
                &host.app,
                crate::keymap::Context::Canvas,
            ));
            assert_eq!(
                before,
                rows(crate::ops::help::project_sections_legacy_for_test(
                    &host.app
                )),
                "project help keeps the HEAD path"
            );
            host.key('?');
            assert_eq!(
                before,
                rows(crate::ops::help::sections_for(
                    &host.app,
                    crate::ops::help::help_context(&host.app)
                ))
            );
            host.key('q');
            host.key(',');
            assert_eq!(
                crate::ops::menu::menu(
                    &host.app,
                    crate::keymap::Context::Canvas,
                    crate::keymap::LeaderId::Options,
                    crate::ops::menu::View::Open
                ),
                crate::ops::menu::preview_options_legacy_for_test(
                    &host.app,
                    crate::keymap::Context::Canvas
                )
            );
            host.esc();
            host.key('q');
            drain(&mut host);
            assert!(!host.app.project_active(), "q closes the project overlay");
            assert_eq!(
                host.app.tab.preview_path.as_deref(),
                Some(root.join("doc.md").as_path())
            );
        }
    }
}

#[test]
fn project_over_preview_restores_diff_and_plain_preview_host() {
    if isolated("project_over_preview_restores_diff_and_plain_preview_host") {
        return;
    }
    for diff in [true, false] {
        let root = at_home(preview_shape_repo("doc.md"));
        let cfg: Config = toml::from_str(&config(
            "git",
            "en",
            "vim",
            "relative",
            "right",
            "[plugins.git]\nread_only=true\n",
        ))
        .unwrap();
        let mut host = Sim::with_config_sized(&root, cfg, 180, 14).with_media();
        host.app.tab.projects.wait_for_result();
        host.app.poll_projects();
        if diff {
            host.key('p');
            drain(&mut host);
            host.enter();
            drain(&mut host);
            host.key('R');
            host.key('R');
        } else {
            host.select("doc.md");
            host.enter();
        }
        host.settle_inline_images("origin Preview", std::time::Duration::from_secs(30));
        host.app.tab.preview_scroll = 2;
        let choice = host.app.diff_view_choice_for_test();
        let returned = host.app.tab.project_return.is_some();
        host.key('p');
        drain(&mut host);
        host.key('q');
        drain(&mut host);
        assert_eq!(
            host.app.preview_from_diff_for_test(),
            diff,
            "p -> q restores the diff Preview flag"
        );
        assert_eq!(host.app.diff_view_choice_for_test(), choice);
        assert_eq!(host.app.tab.preview_scroll, 2);
        assert_eq!(host.app.tab.project_return.is_some(), returned);
        if diff {
            host.key('R');
            assert!(host.app.is_git_diff_preview());
            host.key('q');
            assert!(host.app.project_active());
        } else {
            assert!(!host.app.preview_is_diff_representation());
        }
    }
}

#[test]
fn project_diff_preview_project_key_matches_host() {
    if isolated("project_diff_preview_project_key_matches_host") {
        return;
    }
    for lang in ["en", "ja"] {
        for width in [48, 180] {
            let roots = diff_repositories(|| {
                let root = sandbox("vcs_app_preview_project_key");
                seed_repo_markdown(&root);
                root
            });
            let text = config(
                "git",
                lang,
                "vim",
                "relative",
                "right",
                "[plugins.git]\nread_only=true\n",
            );
            let launches = Arc::new(Mutex::new(Vec::new()));
            run_roots_with(
                (&roots.host, &roots.app),
                "git",
                &text,
                vec![
                    Step::Resize(width, 14),
                    key(KeyCode::Enter),
                    ch('R'),
                    ch('R'),
                    ch('p'),
                    ch('q'),
                    Step::BeforeKey(
                        Box::new(|root| {
                            std::fs::write(root.join("doc.md"), "# Title\n\nREFRESHED paragraph.\n\n## Section\n\nUnchanged paragraph.\n").unwrap();
                        }),
                        KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE),
                    ),
                    ch('!'),
                    ch('R'),
                    ch('q'),
                ],
                false,
                RunOptions {
                    launches: Some(launches.clone()),
                    ..Default::default()
                },
                |_, _, _, _| {},
            );
            let launches = launches.lock().unwrap();
            assert_eq!(launches.len(), 1);
            assert_eq!(launches[0].program, "lazygit");
            assert!(launches[0].args.is_empty());
        }
    }
}

#[test]
fn project_diff_preview_front_overlays_keep_host_plans() {
    if isolated("project_diff_preview_front_overlays_keep_host_plans") {
        return;
    }
    for lang in ["en", "ja"] {
        for width in [48, 180] {
            for overlay in ['?', ',', ' ', '/', 'm', '\'', 'Q'] {
                let root = at_home(preview_shape_repo("doc.md"));
                let cfg: Config = toml::from_str(&config(
                    "git",
                    lang,
                    "vim",
                    "relative",
                    "right",
                    "[plugins.git]\nread_only=true\n",
                ))
                .unwrap();
                let mut host = Sim::with_config_sized(&root, cfg, width, 14).with_media();
                host.app.tab.projects.wait_for_result();
                host.app.poll_projects();
                host.key('p');
                drain(&mut host);
                host.enter();
                drain(&mut host);
                host.key('R');
                host.key('R');
                host.settle_inline_images("Preview overlay", std::time::Duration::from_secs(30));
                host.key(overlay);
                match overlay {
                    '?' => assert!(host.app.show_help),
                    ',' => assert_eq!(
                        host.app.pending_leader,
                        Some(crate::keymap::LeaderId::Options)
                    ),
                    ' ' => assert_eq!(
                        host.app.pending_leader,
                        Some(crate::keymap::LeaderId::Actions)
                    ),
                    '/' => assert!(host.app.is_searching()),
                    'm' => assert!(host.app.is_marking()),
                    '\'' => assert!(host.app.is_bookmark_list()),
                    'Q' => assert!(host.app.dialog_is_confirm()),
                    _ => unreachable!(),
                }
                let ctx = host.app.key_context().unwrap_or(host.app.base_context());
                if !matches!(overlay, '?' | ',' | ' ') {
                    assert!(
                        crate::ops::preview_plan::front_values(&host.app, host.app.base_context())
                            .is_none(),
                        "a retained Preview is behind {overlay:?}"
                    );
                    let sections = |s: Vec<crate::ui::help::HelpSection>| {
                        s.into_iter().map(|s| (s.title, s.rows)).collect::<Vec<_>>()
                    };
                    assert_eq!(
                        sections(crate::ops::help::sections_for(&host.app, ctx)),
                        sections(crate::ops::help::preview_sections_legacy_for_test(
                            &host.app, ctx
                        ))
                    );
                    assert_eq!(
                        crate::ops::menu::menu(
                            &host.app,
                            ctx,
                            crate::keymap::LeaderId::Options,
                            crate::ops::menu::View::Open
                        ),
                        crate::ops::menu::preview_options_legacy_for_test(&host.app, ctx)
                    );
                }
                assert_eq!(
                    crate::ops::footer::tokens(&host.app, ctx),
                    crate::ops::footer::render(
                        &host.app,
                        ctx,
                        &crate::ops::footer::plan(&host.app, ctx)
                    ),
                    "front footer for {overlay:?} {lang} {width}"
                );
                if crate::ops::core_help::core_ctx(ctx).is_none() {
                    assert!(crate::ops::preview_plan::front_values(&host.app, ctx).is_none());
                }
            }
        }
    }
}

#[test]
fn project_diff_preview_change_navigation_matches_host() {
    if isolated("project_diff_preview_change_navigation_matches_host") {
        return;
    }
    for lang in ["en", "ja"] {
        for width in [48, 180] {
            let roots = diff_repositories(|| {
                let root = sandbox("vcs_app_preview_changes");
                seed_repo_markdown(&root);
                let old = (0..60)
                    .map(|i| format!("Paragraph {i}.\n\n"))
                    .collect::<String>();
                std::fs::write(root.join("doc.md"), &old).unwrap();
                crate::test_support::run_git(&root, &["add", "doc.md"]);
                crate::test_support::run_git(&root, &["commit", "-qm", "paragraph baseline"]);
                std::fs::write(
                    root.join("doc.md"),
                    old.replace("Paragraph 5.", "CHANGED 5.")
                        .replace("Paragraph 40.", "CHANGED 40."),
                )
                .unwrap();
                root
            });
            let text = config(
                "git",
                lang,
                "vim",
                "relative",
                "right",
                "[plugins.git]\nread_only=true\n",
            );
            run_roots_with(
                (&roots.host, &roots.app),
                "git",
                &text,
                vec![
                    Step::Resize(width, 14),
                    key(KeyCode::Enter),
                    ch('R'),
                    ch('R'),
                    ch('n'),
                    ch('n'),
                    ch('N'),
                    Step::Resize(54, 14),
                    ch('n'),
                    ch('q'),
                ],
                false,
                RunOptions {
                    launches: Some(Arc::new(Mutex::new(Vec::new()))),
                    ..Default::default()
                },
                |step, state, _, _| {
                    if (4..=9).contains(&step) {
                        assert_eq!(
                            state.diff.as_ref().unwrap().scroll,
                            match step {
                                4 => 6,
                                5 | 7 | 8 => 9,
                                6 | 9 => 79,
                                _ => unreachable!(),
                            },
                            "n/N scroll at step {step}"
                        );
                    }
                },
            );
        }
    }
}

#[test]
fn project_diff_text_and_code_preview_change_navigation_matches_host() {
    if isolated("project_diff_text_and_code_preview_change_navigation_matches_host") {
        return;
    }
    for lang in ["en", "ja"] {
        for width in [48, 180] {
            for path in ["raw.txt", "raw.rs"] {
                let roots = diff_repositories(|| {
                    let root = preview_shape_repo("doc.md");
                    let old = (0..60)
                        .map(|i| format!("let value_{i} = {i};\n"))
                        .collect::<String>();
                    std::fs::write(root.join(path), &old).unwrap();
                    crate::test_support::run_git(&root, &["add", path]);
                    crate::test_support::run_git(&root, &["commit", "-qm", "raw baseline"]);
                    std::fs::write(
                        root.join(path),
                        old.replace("value_5 =", "changed_5 =")
                            .replace("value_40 =", "changed_40 ="),
                    )
                    .unwrap();
                    root
                });
                // Navigation and caret use Code's real window; disable the unrelated cold grammar worker.
                let text = config(
                    "git",
                    lang,
                    "vim",
                    "relative",
                    "right",
                    "[plugins.git]\nread_only=true\n[plugins.code]\nsyntax_highlight=false\n",
                );
                let mut steps = vec![Step::Resize(width, 14), ch('/')];
                steps.extend(typed(path));
                steps.extend([
                    key(KeyCode::Enter),
                    key(KeyCode::Enter),
                    ch('R'),
                    ch('n'),
                    ch('n'),
                    ch('N'),
                    Step::Resize(54, 14),
                    ch('n'),
                    ch('q'),
                ]);
                run_roots_with(
                    (&roots.host, &roots.app),
                    "git",
                    &text,
                    steps,
                    false,
                    RunOptions {
                        launches: Some(Arc::new(Mutex::new(Vec::new()))),
                        ..Default::default()
                    },
                    |step, state, _, _| {
                        let opened = path.chars().count() + 5;
                        if (opened..=opened + 5).contains(&step) {
                            let diff = state.diff.as_ref().unwrap();
                            let later = matches!(step - opened, 1 | 3 | 4);
                            assert_eq!(diff.first_change, Some(5));
                            assert_eq!(
                                diff.scroll,
                                if later { 37 } else { 2 },
                                "raw n/N scroll step {step}"
                            );
                            assert_eq!(
                                diff.editor_line,
                                Some(if later { 41 } else { 6 }),
                                "raw n/N e line step {step}"
                            );
                        }
                    },
                );
            }
        }
    }
}

fn host_diff_snapshot(host: &mut Sim) -> Option<konoma_plugin::app::project::ProjectDiffSnapshot> {
    use konoma_host_core::project::DiffView;
    use konoma_plugin::app::project::ProjectDiffSnapshot;
    if host.app.project_active() || host.app.is_git_view() || host.app.tab.project_return.is_none()
    {
        return None;
    }
    let path = host.app.tab.preview_path.clone()?;
    let preview = host.app.preview_from_diff_for_test();
    let effective = if preview {
        DiffView::Preview
    } else {
        host.app.diff_view_for_test()
    };
    let width = host.term.backend().buffer().area.width;
    let first_change = if preview {
        if host.app.base_context() == crate::keymap::Context::Text {
            host.app.project_preview_first_mark_for_test()
        } else {
            host.app.md_diff_marks_for_test().and_then(|marks| {
                marks
                    .first()
                    .map(|(range, _)| host.app.md_visual_span_for_test(range.start).0)
            })
        }
    } else {
        host.app.diff_block_rows_for_test().first().copied()
    };
    Some(ProjectDiffSnapshot {
        help: host
            .app
            .show_help
            .then(|| konoma_plugin::app::project::ProjectHelpSnapshot {
                sections: crate::ops::help::sections_for(
                    &host.app,
                    crate::ops::help::help_context(&host.app),
                )
                .into_iter()
                .map(|s| (s.title, s.rows))
                .collect(),
                scroll: host.app.help_scroll as usize,
                title: crate::ops::core_help::help_title(&host.app),
            }),
        baseline_text: (effective != DiffView::Source)
            .then(|| host.app.project_diff_baseline_text_for_test())
            .flatten(),
        preview_context: preview
            .then(|| format!("{:?}", crate::ops::help::help_context(&host.app))),
        path,
        baseline: match host.app.diff_vcs() {
            crate::vcs::VcsKind::Git => "HEAD",
            crate::vcs::VcsKind::Jj => "@-",
        }
        .into(),
        choice: host.app.diff_view_choice_for_test(),
        effective,
        next_representation: host.app.diff_view_cycle_hint().map(|hint| match hint {
            crate::i18n::Msg::DiffViewSource => DiffView::Source,
            crate::i18n::Msg::DiffViewRendered => DiffView::Rendered,
            crate::i18n::Msg::DiffViewPreview => DiffView::Preview,
            _ => unreachable!(),
        }),
        layout: format!("{:?}", host.app.source_diff_layout()),
        resolved_split: host.app.diff_is_split(width.saturating_sub(2)),
        split_drawn: (effective == DiffView::Source).then_some(host.app.diff_split_drawn),
        wrap: host.app.cfg.ui.wrap,
        scroll: if host.app.is_windowed() {
            host.app.preview_top_line()
        } else {
            usize::from(host.app.tab.preview_scroll)
        },
        hscroll: host.app.tab.preview_hscroll,
        first_change,
        editor_line: host.app.preview_edit_line_for_test(),
        return_to: host
            .app
            .tab
            .project_return
            .as_ref()
            .unwrap()
            .stack
            .iter()
            .map(|f| f.key.screen.clone())
            .collect(),
        source_lines: if preview {
            Vec::new()
        } else {
            host.app
                .git_diff_lines()
                .iter()
                .map(|l| (format!("{:?}", l.kind), l.old_no, l.new_no, l.text.clone()))
                .collect()
        },
    })
}

struct DiffEntryContract(&'static str);
impl Plugin for DiffEntryContract {
    fn manifest(&self) -> konoma_plugin::Manifest {
        use konoma_plugin::prelude::*;
        let mut project = ProjectDecl::new()
            .marker(DetectionMarker::new(".git", MarkerKind::Any))
            .version_control();
        for screen in CONTRACT_ROOTS {
            project = project.screen(ProjectScreen::new(screen, screen).row_kind("file"));
        }
        project = project.screen(
            ProjectScreen::new("inspect", "Inspect")
                .pushed()
                .row_kind("file"),
        );
        let mut manifest = Manifest::new("git", "0").project(project);
        for screen in CONTRACT_ROOTS.into_iter().chain(["inspect"]) {
            manifest = manifest.action(
                Act::new(format!("diff_{screen}"))
                    .key("D")
                    .label(konoma_plugin::t!(en: "diff", ja: "差分"))
                    .footer(4)
                    .project_context(ActContext::screen(screen, ["file"])),
            );
            if screen != "inspect" {
                manifest = manifest.action(
                    Act::new(format!("push_{screen}"))
                        .key("Enter")
                        .label(konoma_plugin::t!(en: "push", ja: "重ねる"))
                        .footer(5)
                        .project_context(ActContext::screen(screen, ["file"])),
                );
            }
        }
        manifest
    }
    fn open(
        &self,
        input: konoma_plugin::Input,
        _: &konoma_plugin::OpenCx,
    ) -> Result<Box<dyn konoma_plugin::Session>, konoma_plugin::Failure> {
        let konoma_plugin::Input::Project { root, .. } = input else {
            unreachable!()
        };
        Ok(Box::new(DiffEntrySession(root.join(self.0))))
    }
}
struct DiffEntrySession(std::path::PathBuf);
impl konoma_plugin::Session for DiffEntrySession {
    fn view(
        &mut self,
        _: &konoma_plugin::ViewReq,
        _: &konoma_plugin::ViewCx,
    ) -> Result<konoma_plugin::View, konoma_plugin::Failure> {
        use konoma_plugin::view::{ListRow, ListView};
        Ok(konoma_plugin::View::List(ListView {
            rows: (0..50)
                .map(|i| {
                    ListRow::new(
                        "file",
                        vec![konoma_plugin::Line::raw(format!("match row {i:02}"))],
                    )
                    .value(format!("row-{i}"))
                })
                .collect(),
            ..Default::default()
        }))
    }
    fn act(
        &mut self,
        action: &str,
        cx: &konoma_plugin::ActCx,
    ) -> Result<konoma_plugin::Outcome, konoma_plugin::Failure> {
        if action.starts_with("diff_") {
            Ok(konoma_plugin::Outcome::OpenDiff {
                path: self.0.clone(),
            })
        } else {
            Ok(konoma_plugin::Outcome::PushScreen {
                screen: "inspect".into(),
                selection: cx
                    .project
                    .as_ref()
                    .and_then(|p| p.selected.as_ref())
                    .map(|r| r.value.clone()),
            })
        }
    }
}

fn contract_source_entries() {
    for lang in ["en", "ja"] {
        for width in [48, 180] {
            for path in ["c-deleted.txt", "d-renamed.txt", "日本語.txt"] {
                let roots = diff_repositories(|| git_repo("vcs_app_contract", true));
                let text = config(
                    "git",
                    lang,
                    "vim",
                    "relative",
                    "right",
                    "[plugins.git]\nread_only=true\n[keys.diff]\nS='cycle_diff_layout'\ns='noop'\n",
                );
                for (index, screen) in CONTRACT_ROOTS.into_iter().enumerate() {
                    let mut steps = vec![Step::Resize(width, 14)];
                    for _ in 0..index {
                        steps.push(key(KeyCode::Tab));
                    }
                    let mut marks = Vec::new();
                    for pushed in [false, true] {
                        if pushed {
                            steps.push(key(KeyCode::Enter));
                        }
                        steps.push(ch('/'));
                        steps.extend(typed("match"));
                        steps.extend([key(KeyCode::Enter), ch('G')]);
                        let before = steps.len();
                        steps.extend([ch('D'), ch('S'), ch('s')]);
                        if path == "c-deleted.txt" {
                            steps.push(ch('R'));
                        }
                        steps.push(ch('q'));
                        marks.push((before, steps.len(), if pushed { "inspect" } else { screen }));
                    }
                    let mut retained = [None, None];
                    run_roots_with(
                        (&roots.host, &roots.app),
                        "git",
                        &text,
                        steps,
                        false,
                        RunOptions {
                            plugin: Some(Arc::new(DiffEntryContract(path))),
                            contract: true,
                            launches: Some(Arc::new(Mutex::new(Vec::new()))),
                            ..Default::default()
                        },
                        |step, state, _, _| {
                            for (i, (before, after, screen)) in marks.iter().enumerate() {
                                if step == *before {
                                    assert_eq!(
                                        state.stack.last().map(String::as_str),
                                        Some(*screen)
                                    );
                                    assert_eq!(state.cursor, 49);
                                    assert_eq!(state.query, "match");
                                    retained[i] = Some(state.clone());
                                }
                                if step == *after {
                                    assert_eq!(Some(state), retained[i].as_ref(), "OpenDiff q restores the entire originating project state: {screen} {lang} {width} {path}");
                                }
                            }
                        },
                    );
                }
            }
        }
    }
}

fn real_git_source_entries() {
    for lang in ["en", "ja"] {
        for width in [48, 180] {
            for path in ["c-deleted.txt", "d-renamed.txt", "日本語.txt"] {
                let roots = diff_repositories(|| git_repo("vcs_app_real_diff_entries", true));
                let text = config("git", lang, "vim", "relative", "right", "[plugins.git]\nread_only=true\n[keys.text]\nZ='navigate:right'\nl='noop'\n[keys.diff]\nS='cycle_diff_layout'\ns='noop'\n");
                let mut steps = vec![Step::Resize(width, 14), ch('/')];
                steps.extend(typed(path));
                steps.push(key(KeyCode::Enter));
                let before = steps.len();
                steps.extend([ch('Z'), ch('S'), ch('s')]);
                if path == "c-deleted.txt" {
                    steps.push(ch('R'));
                }
                steps.push(ch('q'));
                let after = steps.len();
                let mut retained = None;
                run_roots_with(
                    (&roots.host, &roots.app),
                    "git",
                    &text,
                    steps,
                    false,
                    RunOptions {
                        diff_entry_remap: true,
                        launches: Some(Arc::new(Mutex::new(Vec::new()))),
                        ..Default::default()
                    },
                    |step, state, _, _| {
                        if step == before {
                            retained = Some(state.clone());
                        }
                        if step == after {
                            assert_eq!(Some(state), retained.as_ref(), "real changes entry restores filter, query insertion point, cursor and target: {path}");
                        }
                    },
                );
            }
        }
    }
}

fn contract_preview_entries() {
    for lang in ["en", "ja"] {
        for width in [48, 180] {
            let roots = diff_repositories(|| {
                let root = sandbox("vcs_app_contract_preview");
                seed_repo_markdown(&root);
                root
            });
            let text = config(
                "git",
                lang,
                "vim",
                "relative",
                "right",
                "[plugins.git]\nread_only=true\n",
            );
            for (index, screen) in CONTRACT_ROOTS.into_iter().enumerate() {
                let mut steps = vec![Step::Resize(width, 14)];
                for _ in 0..index {
                    steps.push(key(KeyCode::Tab));
                }
                let mut marks = Vec::new();
                for pushed in [false, true] {
                    if pushed {
                        steps.push(key(KeyCode::Enter));
                    }
                    steps.push(ch('/'));
                    steps.extend(typed("match"));
                    steps.extend([
                        key(KeyCode::Left),
                        key(KeyCode::Left),
                        key(KeyCode::Enter),
                        ch('G'),
                    ]);
                    for return_through_source in [true, false] {
                        let before = steps.len();
                        steps.extend([ch('D'), ch('R'), ch('R'), ch('j'), ch('k')]);
                        if return_through_source {
                            steps.push(ch('R'));
                        }
                        steps.push(ch('q'));
                        marks.push((before, steps.len()));
                    }
                }
                let mut retained = [None, None, None, None];
                run_roots_with(
                    (&roots.host, &roots.app),
                    "git",
                    &text,
                    steps,
                    false,
                    RunOptions {
                        plugin: Some(Arc::new(DiffEntryContract("doc.md"))),
                        contract: true,
                        launches: Some(Arc::new(Mutex::new(Vec::new()))),
                        ..Default::default()
                    },
                    |step, state, _, _| {
                        for (i, (before, after)) in marks.iter().enumerate() {
                            if step == *before {
                                assert_eq!(state.query, "match");
                                assert_eq!(state.query_cursor, 5, "the project filter keeps its input at the end (Left does not move it)");
                                assert_eq!(state.cursor, 49);
                                retained[i] = Some(state.clone());
                            }
                            if step == *after {
                                assert_eq!(
                                    Some(state),
                                    retained[i].as_ref(),
                                    "Preview R/q restores {screen}: {lang} {width}"
                                );
                            }
                        }
                    },
                );
            }
        }
    }
}

fn diff_repositories(seed: impl Fn() -> crate::test_support::TmpDir) -> WriteRepositories {
    let parent = at_home(sandbox("vcs_app_diff"));
    let roots = WriteRepositories {
        host: parent.join("host/repo"),
        app: parent.join("apps/repo"),
    };
    for root in [&roots.host, &roots.app] {
        std::fs::create_dir_all(root.parent().unwrap()).unwrap();
        std::fs::rename(seed(), root).unwrap();
    }
    roots
}

#[test]
fn project_diff_discard_confirm_returns_refreshes_and_keeps_flash() {
    if isolated("project_diff_discard_confirm_returns_refreshes_and_keeps_flash") {
        return;
    }
    let root = at_home(git_repo("vcs_app_host_discard_return", true));
    let cfg = super::vcs_changes::plugin_config("git", "discard_project_return", "en");
    let mut host = Sim::with_config_sized(&root, cfg, 180, 14).with_async_git_ops();
    host.app.tab.projects.wait_for_result();
    host.app.poll_projects();
    host.key('p');
    drain(&mut host);
    host.key('/');
    for c in ".txt".chars() {
        host.key(c);
    }
    host.enter();
    host.key('j');
    let before = host_state(&host);
    assert_eq!(
        before.selected.as_ref().unwrap().name.as_deref(),
        Some("b-unstaged.txt")
    );
    host.enter();
    drain(&mut host);
    host.key('q');
    drain(&mut host);
    assert_eq!(
        host_state(&host),
        before,
        "q preserves the originating project state"
    );
    host.enter();
    drain(&mut host);
    host.key('x');
    host.key('y');
    host.drain_git_ops();
    drain(&mut host);
    assert!(
        host.app.project_active(),
        "discard from a project diff returns to the project screen"
    );
    assert!(
        !host.app.is_git_view(),
        "the legacy hub must not replace the project screen"
    );
    let after = host_state(&host);
    assert_eq!(after.stack, before.stack);
    assert_eq!(after.query, before.query);
    assert_eq!(after.query_cursor, before.query_cursor);
    assert_eq!(
        after.cursor, 0,
        "a vanished selected identity uses the q path's first selectable row"
    );
    assert_eq!(
        after.selected.as_ref().unwrap().name.as_deref(),
        Some("a-staged.txt")
    );
    assert!(after.notice.as_deref().unwrap().starts_with("Discarded: "));
    assert!(after.notice.as_deref().unwrap().ends_with("b-unstaged.txt"));
    assert_eq!(
        std::fs::read(root.join("b-unstaged.txt")).unwrap(),
        b"base\n"
    );
}

#[test]
fn rendered_discard_hint_and_key_match_host() {
    if isolated("rendered_discard_hint_and_key_match_host") {
        return;
    }
    let roots = diff_repositories(|| {
        let root = sandbox("vcs_app_diff_discard");
        seed_repo_markdown(&root);
        root
    });
    let text = config("git", "en", "vim", "relative", "right", "");
    run_roots_with(
        (&roots.host, &roots.app),
        "git",
        &text,
        vec![
            Step::Resize(180, 14),
            key(KeyCode::Enter),
            ch('R'),
            ch('x'),
            ch('n'),
            ch('x'),
            Step::Resize(54, 14),
            ch('y'),
        ],
        false,
        RunOptions {
            launches: Some(Arc::new(Mutex::new(Vec::new()))),
            ..Default::default()
        },
        |_, _, _, _| {},
    );
}

#[test]
fn rendered_discard_key_remap_matches_host() {
    if isolated("rendered_discard_key_remap_matches_host") {
        return;
    }
    for lang in ["en", "ja"] {
        let roots = diff_repositories(|| {
            let root = sandbox("vcs_app_diff_discard_remap");
            seed_repo_markdown(&root);
            root
        });
        let text = config(
            "git",
            lang,
            "vim",
            "relative",
            "right",
            "[keys.diff]\nx='noop'\nz='git_diff_discard'\n",
        );
        run_roots_with(
            (&roots.host, &roots.app),
            "git",
            &text,
            vec![
                Step::Resize(180, 14),
                key(KeyCode::Enter),
                ch('R'),
                ch('x'),
                ch('z'),
                key(KeyCode::Esc),
                ch('q'),
            ],
            false,
            RunOptions {
                launches: Some(Arc::new(Mutex::new(Vec::new()))),
                ..Default::default()
            },
            |_, _, _, _| {},
        );
    }
}

#[test]
fn project_diff_state_is_compared_at_every_fence() {
    if isolated("project_diff_state_is_compared_at_every_fence") {
        return;
    }
    let roots = diff_repositories(|| git_repo("vcs_app_diff_state", true));
    let text = config("git", "en", "vim", "relative", "right", "[plugins.git]\nread_only=true\n[keys.diff]\nS='cycle_diff_layout'\ns='noop'\n[editor]\ncommand='fixture-editor --line={line}'\n");
    run_roots_with(
        (&roots.host, &roots.app),
        "git",
        &text,
        vec![
            key(KeyCode::Enter),
            ch('S'),
            ch('S'),
            Step::Resize(180, 14),
            ch('n'),
            ch('e'),
            ch(','),
            ch('w'),
            ch('q'),
            key(KeyCode::Enter),
        ],
        false,
        RunOptions {
            launches: Some(Arc::new(Mutex::new(Vec::new()))),
            ..Default::default()
        },
        |step, state, _, _| {
            if step == 1 {
                let diff = state
                    .diff
                    .as_ref()
                    .expect("the active diff is observed independently");
                assert_eq!(diff.baseline, "HEAD");
                assert_eq!(
                    diff.old_bytes(),
                    read_snapshot_for_test("vcs_app_diff")["git_staged_baseline"]
                        .as_str()
                        .unwrap()
                        .as_bytes()
                );
                assert_eq!(diff.editor_line, Some(1));
            }
        },
    );
}

#[test]
fn jj_project_diff_baselines_match_host() {
    if !crate::vcs::jj::available() {
        eprintln!(
            "NOT RUN: U8f: 36 real jj-only/colocated sessions (32 jj, 4 Git); jj is unavailable"
        );
    }
    if !crate::vcs::jj::available() {
        return;
    }
    if isolated("jj_project_diff_baselines_match_host") {
        return;
    }
    jj_diff_cases();
    report_cases();
}

fn jj_diff_cases() {
    for colocated in [false, true] {
        for id in if colocated {
            vec!["jj", "git"]
        } else {
            vec!["jj"]
        } {
            for lang in ["en", "ja"] {
                for width in [48, 180] {
                    let roots = diff_repositories(|| {
                        let root = sandbox("vcs_app_jj_diff");
                        super::vcs_changes::seed_jj_write_repository(&root, colocated);
                        root
                    });
                    let text = config(id, lang, "vim", "relative", "right", &format!("[plugins.{id}]\nread_only=true\n[keys.diff]\nS='cycle_diff_layout'\ns='noop'\n[editor]\ncommand='fixture-editor --line={{line}}'\n"));
                    run_roots_with(
                        (&roots.host, &roots.app),
                        id,
                        &text,
                        vec![
                            Step::Resize(width, 14),
                            key(KeyCode::Enter),
                            ch('S'),
                            ch('S'),
                            Step::Resize(180, 14),
                            ch('n'),
                            ch('e'),
                            ch('q'),
                        ],
                        false,
                        RunOptions {
                            launches: Some(Arc::new(Mutex::new(Vec::new()))),
                            ..Default::default()
                        },
                        |step, state, _, _| {
                            if step == 2 {
                                let diff = state.diff.as_ref().unwrap();
                                assert_eq!(diff.baseline, if id == "jj" { "@-" } else { "HEAD" });
                                assert_eq!(
                                    diff.old_bytes(),
                                    read_snapshot_for_test("vcs_app_diff")["jj_parent_baseline"]
                                        .as_str()
                                        .unwrap()
                                        .as_bytes()
                                );
                            }
                        },
                    );
                }
            }
        }
    }
    jj_rendered_preview_cases();
    jj_detail_entry_cases();
}

fn jj_detail_entry_cases() {
    fn copy_fixture(source: &Path, target: &Path) {
        std::fs::create_dir_all(target).unwrap();
        for entry in std::fs::read_dir(source).unwrap() {
            let entry = entry.unwrap();
            let next = target.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                copy_fixture(&entry.path(), &next);
            } else {
                std::fs::copy(entry.path(), next).unwrap();
            }
        }
    }
    for colocated in [false, true] {
        // Jj generates random change IDs. Both independent repositories are copied from a
        // pristine fixture that neither tested host ever opens or writes, before either runs.
        let seed = sandbox("vcs_app_jj_detail_seed");
        super::vcs_changes::seed_jj_write_repository(&seed, colocated);
        for lang in ["en", "ja"] {
            for width in [48, 180] {
                for graph in [false, true] {
                    let roots = diff_repositories(|| {
                        let root = sandbox("vcs_app_jj_detail_entry");
                        copy_fixture(&seed, &root);
                        root
                    });
                    let text = config("jj", lang, "vim", "relative", "right", "[plugins.jj]\nread_only=true\n[keys.text]\nD='navigate:down'\n[keys.diff]\nS='cycle_diff_layout'\ns='noop'\n");
                    let mut steps = vec![Step::Resize(width, 14), key(KeyCode::Tab)];
                    if graph {
                        steps.push(key(KeyCode::Tab));
                    }
                    steps.push(ch('G'));
                    let before = steps.len();
                    steps.extend([
                        key(KeyCode::Enter),
                        ch('S'),
                        ch('s'),
                        Step::Resize(if width == 48 { 180 } else { 48 }, 14),
                        ch('q'),
                    ]);
                    let after = steps.len();
                    let mut retained = None;
                    run_roots_with(
                        (&roots.host, &roots.app),
                        "jj",
                        &text,
                        steps,
                        false,
                        RunOptions::default(),
                        |step, state, host, _| {
                            if step == before {
                                retained = Some(state.clone());
                            }
                            if step == before + 1 {
                                assert_eq!(state.stack.last().map(String::as_str), Some("detail"));
                                assert!(matches!(
                                    host.app.project_content().as_deref(),
                                    Some(ProjectContent::Diff(_))
                                ));
                            }
                            if step == after {
                                let original = retained.as_ref().unwrap();
                                assert_eq!((&state.stack, &state.selected, state.cursor, state.scroll, &state.query, state.filtering), (&original.stack, &original.selected, original.cursor, original.scroll, &original.query, original.filtering), "the declared jj detail entry returns to its original log/graph row");
                            }
                        },
                    );
                }
            }
        }
    }
}

fn jj_rendered_preview_cases() {
    for colocated in [false, true] {
        for lang in ["en", "ja"] {
            for width in [48, 180] {
                let roots = diff_repositories(|| {
                    let root = sandbox("vcs_app_jj_markdown_diff");
                    super::vcs_changes::seed_jj_write_repository(&root, colocated);
                    let old = read_snapshot_for_test("vcs_app_diff")["markdown_head_baseline"]
                        .as_str()
                        .unwrap()
                        .to_string();
                    std::fs::write(root.join("doc.md"), &old).unwrap();
                    jj(&root, &["new", "-m", "preview parent"]);
                    std::fs::write(
                        root.join("doc.md"),
                        format!(
                            "{}\n## New Section\n\nBrand new content.\n",
                            old.replace("Original paragraph.", "CHANGED paragraph.")
                        ),
                    )
                    .unwrap();
                    root
                });
                let text = config(
                    "jj",
                    lang,
                    "vim",
                    "relative",
                    "right",
                    "[plugins.jj]\nread_only=true\n",
                );
                let launches = Arc::new(Mutex::new(Vec::new()));
                run_roots_with(
                    (&roots.host, &roots.app),
                    "jj",
                    &text,
                    vec![
                        Step::Resize(width, 14),
                        key(KeyCode::Enter),
                        ch('R'),
                        ch('R'),
                        ch('p'),
                        ch('q'),
                        ch('r'),
                        ch('!'),
                        ch('q'),
                    ],
                    false,
                    RunOptions {
                        launches: Some(launches.clone()),
                        ..Default::default()
                    },
                    |step, state, _, _| {
                        if (2..=4).contains(&step) {
                            let diff = state.diff.as_ref().unwrap();
                            assert_eq!(diff.baseline, "@-");
                            let old = if step == 2 {
                                diff.old_bytes()
                            } else {
                                diff.baseline_text.as_deref().unwrap().as_bytes().to_vec()
                            };
                            assert_eq!(
                                old,
                                read_snapshot_for_test("vcs_app_diff")["markdown_head_baseline"]
                                    .as_str()
                                    .unwrap()
                                    .as_bytes(),
                                "jj @- bytes in each representation"
                            );
                        }
                    },
                );
                let launches = launches.lock().unwrap();
                assert_eq!(launches.len(), 1);
                assert_eq!(launches[0].program, "lazyjj");
                assert!(launches[0].args.is_empty());
            }
        }
    }
}

#[test]
fn diff_choices_and_defaults_match_host() {
    if isolated("diff_choices_and_defaults_match_host") {
        return;
    }
    for lang in ["en", "ja"] {
        for width in [48, 180] {
            for wrap in [false, true] {
                let roots = diff_repositories(|| {
                    let root = sandbox("vcs_app_choice");
                    seed_repo_markdown(&root);
                    root
                });
                let text = config(
                    "git",
                    lang,
                    "vim",
                    "relative",
                    "right",
                    "[plugins.git]\nread_only=true\n",
                )
                .replace("keys='vim'", &format!("keys='vim'\nwrap={wrap}"))
                .replace("diff='unified'", "diff='auto'");
                run_roots_with(
                    (&roots.host, &roots.app),
                    "git",
                    &text,
                    vec![
                        Step::Resize(width, 14),
                        key(KeyCode::Enter),
                        ch('R'),
                        ch('q'),
                        key(KeyCode::Enter),
                        ch('q'),
                    ],
                    false,
                    RunOptions {
                        launches: Some(Arc::new(Mutex::new(Vec::new()))),
                        ..Default::default()
                    },
                    |step, state, _, _| {
                        use konoma_host_core::project::DiffView;
                        if step == 3 {
                            assert_eq!(state.diff.as_ref().unwrap().choice, DiffView::Rendered);
                        }
                        if step == 5 {
                            let diff = state.diff.as_ref().unwrap();
                            assert_eq!(
                                diff.choice,
                                DiffView::Source,
                                "a fresh OpenDiff uses the configured default"
                            );
                            assert_eq!(diff.effective, DiffView::Source);
                        }
                    },
                );
                let rendered = text.replace("diff_view='source'", "diff_view='rendered'");
                let mut steps = vec![Step::Resize(width, 14), ch('/')];
                steps.extend(typed("plain.txt"));
                steps.extend([key(KeyCode::Enter), key(KeyCode::Enter)]);
                let opened = steps.len();
                steps.extend([
                    Step::Key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL)),
                    Step::Key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL)),
                    ch('q'),
                ]);
                run_roots_with(
                    (&roots.host, &roots.app),
                    "git",
                    &rendered,
                    steps,
                    false,
                    RunOptions {
                        launches: Some(Arc::new(Mutex::new(Vec::new()))),
                        ..Default::default()
                    },
                    |step, state, _, _| {
                        use konoma_host_core::project::DiffView;
                        if (opened..=opened + 2).contains(&step) {
                            let diff = state.diff.as_ref().unwrap();
                            assert_eq!(
                                diff.choice,
                                DiffView::Rendered,
                                "fallback must retain the chosen representation"
                            );
                            assert_eq!(
                                diff.effective,
                                if step == opened {
                                    DiffView::Source
                                } else {
                                    DiffView::Rendered
                                }
                            );
                            if step == opened {
                                assert_eq!(diff.old_bytes(), b"line one\nline two\n");
                            }
                        }
                    },
                );
            }
        }
    }
    report_cases();
}

#[test]
fn real_detail_and_file_diff_layout_remap_match_host() {
    if isolated("real_detail_and_file_diff_layout_remap_match_host") {
        return;
    }
    let roots = diff_repositories(|| {
        let root = detail_repo();
        std::fs::write(root.join("main.rs"), "changed\n").unwrap();
        root
    });
    for lang in ["en", "ja"] {
        let text = config("git", lang, "vim", "relative", "right", "[plugins.git]\nread_only=true\n[keys.text]\nD='navigate:down'\n[keys.diff]\nS='cycle_diff_layout'\ns='noop'\n[keys.'git.detail']\nS='noop'\n");
        for graph in [false, true] {
            let mut steps = vec![key(KeyCode::Tab)];
            if graph {
                steps.push(key(KeyCode::Tab));
            }
            steps.extend([
                key(KeyCode::Enter),
                ch('S'),
                ch('s'),
                ch('q'),
                ch('p'),
                key(KeyCode::Enter),
                ch('S'),
                ch('s'),
                ch('q'),
            ]);
            run_roots_with(
                (&roots.host, &roots.app),
                "git",
                &text,
                steps,
                false,
                RunOptions {
                    launches: Some(Arc::new(Mutex::new(Vec::new()))),
                    ..Default::default()
                },
                |_, _, _, _| {},
            );
        }
    }
    report_cases();
}

#[test]
fn open_diff_from_each_project_surface_restores_its_state() {
    if isolated("open_diff_from_each_project_surface_restores_its_state") {
        return;
    }
    let roots = diff_repositories(|| git_repo("vcs_app_diff_seed", true));
    let text = config("git", "en", "vim", "relative", "right", "");
    run_roots_with(
        (&roots.host, &roots.app),
        "git",
        &text,
        vec![Step::Resize(180, 14), key(KeyCode::Enter), ch('q')],
        false,
        RunOptions {
            launches: Some(Arc::new(Mutex::new(Vec::new()))),
            ..Default::default()
        },
        |step, state, host, _| {
            if step == 2 {
                assert!(host.app.is_git_diff_preview());
            }
            if step == 3 {
                assert_eq!(state.stack, ["changes"]);
            }
        },
    );
    contract_preview_entries();
    contract_source_entries();
    real_git_source_entries();
    worktree_failure_and_diff_cases();
    report_cases();
}

#[test]
fn project_diff_preview_help_options_and_wrap_return_match_host() {
    if isolated("project_diff_preview_help_options_and_wrap_return_match_host") {
        return;
    }
    for lang in ["en", "ja"] {
        for width in [48, 180] {
            let roots = diff_repositories(|| {
                let root = sandbox("vcs_app_preview_options");
                seed_repo_markdown(&root);
                root
            });
            let text = config(
                "git",
                lang,
                "vim",
                "relative",
                "right",
                "[plugins.git]\nread_only=true\n",
            );
            run_roots_with(
                (&roots.host, &roots.app),
                "git",
                &text,
                vec![
                    Step::Resize(width, 14),
                    key(KeyCode::Enter),
                    ch('R'),
                    ch('R'),
                    ch(','),
                    ch('w'),
                    ch('R'),
                    ch('R'),
                    ch('R'),
                    ch('?'),
                    ch('q'),
                    ch(','),
                    key(KeyCode::Esc),
                    ch('q'),
                    key(KeyCode::Enter),
                    ch('q'),
                ],
                false,
                RunOptions {
                    launches: Some(Arc::new(Mutex::new(Vec::new()))),
                    ..Default::default()
                },
                |step, state, _, _| {
                    if matches!(step, 7 | 15) {
                        assert!(
                            !state.diff.as_ref().unwrap().wrap,
                            "Preview's wrap setting survives R and a fresh OpenDiff"
                        );
                    }
                },
            );
        }
    }
}

#[test]
fn nontrivial_source_change_positions_and_editor_lines_match_host() {
    if isolated("nontrivial_source_change_positions_and_editor_lines_match_host") {
        return;
    }
    for lang in ["en", "ja"] {
        for layout in ["unified", "split", "auto"] {
            let roots = diff_repositories(|| {
                let root = git_repo("vcs_app_diff_deep_lines", false);
                let old: String = (1..=120).map(|line| format!("line {line}\n")).collect();
                std::fs::write(root.join("a-staged.txt"), &old).unwrap();
                crate::test_support::run_git(&root, &["add", "a-staged.txt"]);
                crate::test_support::run_git(&root, &["commit", "-qm", "numbered lines"]);
                let indexed = old.replace("line 20\n", "indexed 20\n");
                std::fs::write(root.join("a-staged.txt"), &indexed).unwrap();
                crate::test_support::run_git(&root, &["add", "a-staged.txt"]);
                std::fs::write(
                    root.join("a-staged.txt"),
                    indexed.replace("line 95\n", "worktree 95\n"),
                )
                .unwrap();
                root
            });
            let text = config(
                "git",
                lang,
                "vim",
                "relative",
                "right",
                "[plugins.git]\nread_only=true\n[editor]\ncommand='fixture-editor --line={line}'\n",
            )
            .replace("diff='unified'", &format!("diff='{layout}'"));
            let launches = Arc::new(Mutex::new(Vec::new()));
            run_roots_with(
                (&roots.host, &roots.app),
                "git",
                &text,
                vec![
                    Step::Resize(48, 14),
                    key(KeyCode::Enter),
                    ch('e'),
                    ch('n'),
                    ch('e'),
                    ch('n'),
                    ch('e'),
                    ch('N'),
                    ch('e'),
                    Step::Resize(180, 14),
                    ch('e'),
                    ch('q'),
                ],
                false,
                RunOptions {
                    launches: Some(launches.clone()),
                    ..Default::default()
                },
                |step, state, _, _| {
                    if step == 2 {
                        let diff = state.diff.as_ref().unwrap();
                        assert_eq!(diff.first_change, Some(3));
                        assert_eq!(diff.scroll, 0);
                        assert_eq!(diff.editor_line, Some(20));
                    }
                    if step == 6 {
                        assert_eq!(state.diff.as_ref().unwrap().editor_line, Some(95));
                    }
                    if step == 8 {
                        assert_eq!(state.diff.as_ref().unwrap().editor_line, Some(20));
                    }
                },
            );
            assert_eq!(
                launches
                    .lock()
                    .unwrap()
                    .iter()
                    .map(|r| r.line)
                    .collect::<Vec<_>>(),
                [Some(20), Some(20), Some(95), Some(20), Some(20)]
            );
        }
    }
}

#[test]
fn rendered_and_preview_baseline_text_matches_expected_bytes() {
    if isolated("rendered_and_preview_baseline_text_matches_expected_bytes") {
        return;
    }
    let roots = diff_repositories(|| {
        let root = sandbox("vcs_app_staged_markdown_baseline");
        seed_repo_markdown(&root);
        let current = std::fs::read(root.join("doc.md")).unwrap();
        std::fs::write(root.join("doc.md"), "INDEX version\n").unwrap();
        crate::test_support::run_git(&root, &["add", "doc.md"]);
        std::fs::write(root.join("doc.md"), current).unwrap();
        root
    });
    let text = config(
        "git",
        "en",
        "vim",
        "relative",
        "right",
        "[plugins.git]\nread_only=true\n",
    );
    run_roots_with(
        (&roots.host, &roots.app),
        "git",
        &text,
        vec![
            Step::Resize(180, 14),
            key(KeyCode::Enter),
            ch('R'),
            ch('R'),
            ch('q'),
        ],
        false,
        RunOptions {
            launches: Some(Arc::new(Mutex::new(Vec::new()))),
            ..Default::default()
        },
        |step, state, _, _| {
            if matches!(step, 3 | 4) {
                assert_eq!(
                    state
                        .diff
                        .as_ref()
                        .unwrap()
                        .baseline_text
                        .as_deref()
                        .unwrap()
                        .as_bytes(),
                    read_snapshot_for_test("vcs_app_diff")["markdown_head_baseline"]
                        .as_str()
                        .unwrap()
                        .as_bytes()
                );
            }
        },
    );
}

#[test]
fn project_diff_statusbar_top_and_bottom_match_host() {
    if isolated("project_diff_statusbar_top_and_bottom_match_host") {
        return;
    }
    for lang in ["en", "ja"] {
        for statusbar in ["top", "bottom"] {
            let roots = diff_repositories(|| {
                let root = sandbox("vcs_app_diff_statusbar");
                seed_repo_markdown(&root);
                root
            });
            let text = config(
                "git",
                lang,
                "vim",
                "relative",
                "right",
                "[plugins.git]\nread_only=true\n",
            )
            .replace("statusbar='split'", &format!("statusbar='{statusbar}'"));
            run_roots_with(
                (&roots.host, &roots.app),
                "git",
                &text,
                vec![
                    Step::Resize(180, 14),
                    key(KeyCode::Enter),
                    ch('R'),
                    ch('R'),
                    Step::Resize(54, 14),
                    ch('R'),
                    ch('q'),
                ],
                false,
                RunOptions {
                    launches: Some(Arc::new(Mutex::new(Vec::new()))),
                    ..Default::default()
                },
                |_, _, _, _| {},
            );
        }
    }
}

#[test]
fn project_diff_preview_shared_help_keeps_host_rows() {
    if isolated("project_diff_preview_shared_help_keeps_host_rows") {
        return;
    }
    for lang in ["en", "ja"] {
        for width in [48, 180] {
            for path in ["doc.md", "image.png", "table.csv"] {
                let root = at_home(preview_shape_repo(path));
                let cfg: Config = toml::from_str(&config(
                    "git",
                    lang,
                    "vim",
                    "relative",
                    "right",
                    "[plugins.git]\nread_only=true\n",
                ))
                .unwrap();
                let mut host = Sim::with_config_sized(&root, cfg, width, 14).with_media();
                host.app.tab.projects.wait_for_result();
                host.app.poll_projects();
                host.key('p');
                drain(&mut host);
                host.key('/');
                for c in path.chars() {
                    host.key(c);
                }
                host.enter();
                host.enter();
                drain(&mut host);
                host.settle_inline_images(
                    "legacy plan baseline",
                    std::time::Duration::from_secs(30),
                );
                match path {
                    "doc.md" => {
                        host.key('R');
                        host.key('R');
                    }
                    "image.png" => host.key('R'),
                    _ => {}
                }
                host.draw();
                host.settle_inline_images(
                    "legacy Preview plan",
                    std::time::Duration::from_secs(30),
                );
                assert_eq!(host.app.preview_from_diff_for_test(), path != "table.csv");
                let context = crate::ops::help::help_context(&host.app);
                if path == "image.png" {
                    assert_eq!(
                        context,
                        crate::keymap::Context::Image,
                        "actual Image Preview"
                    );
                }
                assert_eq!(
                    crate::ops::footer::tokens(&host.app, context),
                    crate::ops::footer::render(
                        &host.app,
                        context,
                        &crate::ops::footer::plan(&host.app, context)
                    ),
                    "the shared footer keeps the host's existing tokens"
                );
                let rows = |sections: Vec<crate::ui::help::HelpSection>| {
                    sections
                        .into_iter()
                        .map(|s| (s.title, s.rows))
                        .collect::<Vec<_>>()
                };
                assert_eq!(
                    rows(crate::ops::help::sections_for(&host.app, context)),
                    rows(crate::ops::help::preview_sections_legacy_for_test(
                        &host.app, context
                    )),
                    "the shared help keeps the host's existing rows"
                );
                assert_eq!(
                    crate::ops::menu::menu(
                        &host.app,
                        context,
                        crate::keymap::LeaderId::Options,
                        crate::ops::menu::View::Open
                    ),
                    crate::ops::menu::preview_options_legacy_for_test(&host.app, context),
                    "Options keeps the host's legacy plan: {lang} {width} {path}"
                );
            }
        }
    }
}

#[test]
fn project_diff_image_preview_and_table_source_chrome_match_host() {
    if isolated("project_diff_image_preview_and_table_source_chrome_match_host") {
        return;
    }
    for path in ["image.png", "table.csv"] {
        for lang in ["en", "ja"] {
            for width in [54, 180] {
                let roots = diff_repositories(|| preview_shape_repo(path));
                let text = config(
                    "git",
                    lang,
                    "vim",
                    "relative",
                    "right",
                    "[plugins.git]\nread_only=true\n",
                );
                let mut steps = vec![Step::Resize(width, 14), ch('/')];
                steps.extend(typed(path));
                steps.extend([
                    key(KeyCode::Enter),
                    key(KeyCode::Enter),
                    ch('R'),
                    Step::Resize(if width == 54 { 180 } else { 54 }, 14),
                    ch('R'),
                    ch('q'),
                ]);
                run_roots_with(
                    (&roots.host, &roots.app),
                    "git",
                    &text,
                    steps,
                    false,
                    RunOptions {
                        pictures: true,
                        launches: Some(Arc::new(Mutex::new(Vec::new()))),
                        ..Default::default()
                    },
                    |step, state, _, _| {
                        use konoma_host_core::project::DiffView;
                        let opened = path.chars().count() + 4;
                        if (opened..=opened + 3).contains(&step) {
                            let diff = state.diff.as_ref().unwrap();
                            if path == "image.png" {
                                assert_eq!(
                                    diff.effective,
                                    if matches!(step, s if s == opened + 1 || s == opened + 2) {
                                        DiffView::Preview
                                    } else {
                                        DiffView::Rendered
                                    }
                                );
                            } else {
                                // The real host declares only Source for Table/Other; R is a no-op.
                                assert_eq!(diff.effective, DiffView::Source);
                                assert_eq!(diff.next_representation, None);
                            }
                        }
                    },
                );
            }
        }
    }
    report_cases();
}

fn preview_shape_repo(path: &str) -> crate::test_support::TmpDir {
    preview_shape_repo_backend(path, "git")
}

fn preview_shape_repo_backend(path: &str, id: &str) -> crate::test_support::TmpDir {
    let root = sandbox("vcs_app_preview_shapes");
    if id == "git" {
        seed_repo_markdown(&root);
    } else {
        super::vcs_changes::seed_jj_write_repository(&root, false);
    }
    if path == "doc.md" {
        if id == "jj" {
            let old = read_snapshot_for_test("vcs_app_diff")["markdown_head_baseline"]
                .as_str()
                .unwrap()
                .to_owned();
            std::fs::write(root.join(path), &old).unwrap();
            jj(&root, &["new", "-m", "preview parent"]);
            std::fs::write(
                root.join(path),
                old.replace("Original paragraph.", "CHANGED paragraph."),
            )
            .unwrap();
        }
        return root;
    }
    let media = matches!(
        path,
        "image.svg"
            | "image.gif"
            | "idle.gif"
            | "pages.pdf"
            | "added.png"
            | "deleted.png"
            | "broken.png"
            | "crop.png"
    );
    if path == "added.png" {
        write_solid_png(&root.join(path), 8, 8, [40, 50, 60]);
        return root;
    }
    if media {
        write_project_media(&root.join(path), false);
    } else if path == "image.png" {
        write_solid_png(&root.join(path), 8, 8, [10, 20, 30]);
    } else if path == "binary.bin" {
        std::fs::write(root.join(path), b"\0old binary").unwrap();
    } else {
        std::fs::write(root.join(path), "name,value\nold,1\n").unwrap();
    }
    if id == "git" {
        crate::test_support::run_git(&root, &["add", path]);
        crate::test_support::run_git(&root, &["commit", "-qm", "preview shape"]);
    } else {
        jj(&root, &["new", "-m", "preview shape"]);
    }
    if media {
        write_project_media(&root.join(path), true);
    } else if path == "image.png" {
        write_solid_png(&root.join(path), 8, 8, [40, 50, 60]);
    } else if path == "binary.bin" {
        std::fs::write(root.join(path), b"\0new binary").unwrap();
    } else {
        std::fs::write(root.join(path), "name,value\nnew,2\n").unwrap();
    }
    root
}

#[test]
fn source_rendered_preview_baselines_and_cycles_match_host() {
    if isolated("source_rendered_preview_baselines_and_cycles_match_host") {
        return;
    }
    let seed = || {
        let root = sandbox("vcs_app_diff_markdown");
        seed_repo_markdown(&root);
        root
    };
    let roots = diff_repositories(seed);
    let text = config(
        "git",
        "en",
        "vim",
        "relative",
        "right",
        "[plugins.git]\nread_only=true\n",
    );
    run_roots_with(
        (&roots.host, &roots.app),
        "git",
        &text,
        vec![
            Step::Resize(180, 14),
            key(KeyCode::Enter),
            Step::Resize(48, 14),
            ch('R'),
            Step::Resize(180, 14),
            ch('R'),
            Step::Resize(54, 14),
            ch('R'),
            Step::Resize(180, 14),
            ch('q'),
        ],
        false,
        RunOptions {
            launches: Some(Arc::new(Mutex::new(Vec::new()))),
            ..Default::default()
        },
        |step, state, host, _| {
            if (2..=9).contains(&step) {
                use konoma_host_core::project::DiffView;
                assert!(!host.app.project_active());
                let diff = state.diff.as_ref().unwrap();
                let (effective, next, hint) = match step {
                    2 | 3 | 8 | 9 => (DiffView::Source, Some(DiffView::Rendered), "R:rendered"),
                    4 | 5 => (DiffView::Rendered, Some(DiffView::Preview), "R:preview"),
                    6 | 7 => (DiffView::Preview, None, "R:back to diff"),
                    _ => unreachable!(),
                };
                assert_eq!(diff.effective, effective);
                assert_eq!(
                    diff.next_representation, next,
                    "the R hint follows the effective representation"
                );
                assert_eq!(diff.baseline, "HEAD");
                let actual = if effective == DiffView::Source {
                    diff.old_bytes()
                } else {
                    diff.baseline_text.as_deref().unwrap().as_bytes().to_vec()
                };
                assert_eq!(
                    actual,
                    read_snapshot_for_test("vcs_app_diff")["markdown_head_baseline"]
                        .as_str()
                        .unwrap()
                        .as_bytes(),
                    "HEAD bytes at step {step}: {effective:?}"
                );
                if matches!(step, 2 | 5 | 6 | 9) {
                    let row = if effective == DiffView::Preview {
                        crate::ops::footer::tokens(
                            &host.app,
                            crate::ops::help::help_context(&host.app),
                        )
                        .join("  ")
                    } else {
                        host.app
                            .diff_presentation()
                            .footer_tokens()
                            .iter()
                            .map(|t| t.text.clone())
                            .collect::<Vec<_>>()
                            .join("  ")
                    };
                    assert!(row.contains(hint), "step {step}: {hint:?} in {row:?}");
                }
            }
            if step == 10 {
                assert_eq!(state.stack, ["changes"]);
            }
        },
    );
}
