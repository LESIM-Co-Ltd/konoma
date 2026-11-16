// Media project entries exercised by the uninterrupted U8f driver.

#[derive(Default)]
struct HelpCoverage(std::collections::BTreeMap<String, Vec<bool>>);
impl HelpCoverage {
    fn observe(&mut self, state: &ProjectStateSnapshot, area: Rect) {
        let Some(diff) = &state.diff else {
            return;
        };
        let Some(help) = &diff.help else {
            return;
        };
        let refs = help
            .sections
            .iter()
            .map(|(title, rows)| (title.as_str(), rows.as_slice()))
            .collect::<Vec<_>>();
        let lines = konoma_widgets::help_panel::help_lines(&refs);
        let (_, visible) = konoma_widgets::help_panel::popup_layout(lines.len(), area);
        let seen = self
            .0
            .entry(format!("{:?}/{:?}", diff.effective, help.sections))
            .or_insert_with(|| vec![false; lines.len()]);
        for row in seen.iter_mut().skip(help.scroll).take(usize::from(visible)) {
            *row = true;
        }
    }
    fn complete(&self) {
        assert!(!self.0.is_empty());
        for (kind, seen) in &self.0 {
            assert!(
                seen.iter().all(|row| *row),
                "every help row must be drawn while paging {kind}; missed {:?}",
                seen.iter()
                    .enumerate()
                    .filter(|(_, seen)| !**seen)
                    .map(|(i, _)| i)
                    .collect::<Vec<_>>()
            );
        }
    }
}

fn expected_diff_actions(host: &[(String, String)], lang: &str) -> Vec<(String, String)> {
    // Design :1266: tabs and the other declared host-only capabilities are absent.
    let sections = vec![(
        String::new(),
        host.iter()
            .map(|(key, label)| (format!("Space {key}"), label.clone()))
            .collect(),
    )];
    expected_preview_help_sections(&sections, lang, false)
        .into_iter()
        .flat_map(|(_, rows)| rows)
        .map(|(key, label)| (key.strip_prefix("Space ").unwrap().into(), label))
        .collect()
}

fn write_project_media(path: &Path, changed: bool) {
    match path.file_name().unwrap().to_str().unwrap() {
        "image.svg" => {
            let (w, h, color) = if changed {
                (20, 40, "blue")
            } else {
                (40, 20, "red")
            };
            std::fs::write(path, format!("<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w}\" height=\"{h}\" viewBox=\"0 0 {w} {h}\"><rect width=\"{w}\" height=\"{h}\" fill=\"{color}\"/></svg>")).unwrap();
        }
        "pages.pdf" => {
            let pages = if changed {
                vec![(80, 30, b"0 0 1 rg 0 0 80 30 re f".as_slice()); 3]
            } else {
                vec![(40, 60, b"1 0 0 rg 0 0 40 60 re f".as_slice()); 2]
            };
            std::fs::write(path, konoma_test_support::build_pdf(&pages)).unwrap();
        }
        "image.gif" | "idle.gif" => {
            use image::{Delay, Frame, Rgba, RgbaImage};
            let mut encoder =
                image::codecs::gif::GifEncoder::new(std::fs::File::create(path).unwrap());
            encoder
                .set_repeat(image::codecs::gif::Repeat::Infinite)
                .unwrap();
            for (index, color) in (if changed {
                [[0, 0, 200, 255], [0, 200, 0, 255]]
            } else {
                [[200, 0, 0, 255], [200, 200, 0, 255]]
            })
            .into_iter()
            .enumerate()
            {
                encoder
                    .encode_frame(Frame::from_parts(
                        RgbaImage::from_pixel(12, 6, Rgba(color)),
                        0,
                        0,
                        Delay::from_numer_denom_ms(
                            if index == 0 && path.ends_with("idle.gif") {
                                0
                            } else {
                                3_600_000
                            },
                            1,
                        ),
                    ))
                    .unwrap();
            }
        }
        "deleted.png" if changed => {
            std::fs::remove_file(path).unwrap();
        }
        "broken.png" if changed => {
            std::fs::write(path, b"broken image").unwrap();
        }
        "crop.png" => {
            let (w, h) = if changed { (80, 240) } else { (320, 60) };
            write_solid_png(
                path,
                w,
                h,
                if changed { [20, 30, 40] } else { [200, 30, 40] },
            );
        }
        _ => write_solid_png(
            path,
            8,
            8,
            if changed { [40, 50, 60] } else { [10, 20, 30] },
        ),
    }
}

#[test]
fn pdf_diff_status_does_not_reuse_the_departed_preview_page() {
    if isolated("pdf_diff_status_does_not_reuse_the_departed_preview_page") {
        return;
    }
    let root = at_home(preview_shape_repo("pages.pdf"));
    let text = config(
        "git",
        "en",
        "vim",
        "relative",
        "right",
        "[plugins.git]\nread_only=true\n",
    );
    let cfg: Config = toml::from_str(&text).unwrap();
    let mut host = Sim::with_config_sized(&root, cfg, 180, 14).with_media();
    host.app.tab.projects.wait_for_result();
    host.app.poll_projects();
    host.key('p');
    drain(&mut host);
    host.key('/');
    host.keys("pages.pdf");
    host.enter();
    host.enter();
    drain(&mut host);
    host.settle_inline_images("fresh PDF diff", ROOT_WAIT);
    let fresh_status = crate::ui::status::context_spans(&host.app);
    host.key('R');
    host.settle_inline_images("PDF Preview", ROOT_WAIT);
    host.key('J');
    host.settle_inline_images("PDF Preview page 2", ROOT_WAIT);
    assert_eq!(host.app.page_indicator(), Some((2, 3)));
    host.key('R');
    host.key('J');
    host.settle_inline_images("PDF diff after Preview", ROOT_WAIT);
    dump_parity_frame("host PDF diff after Preview, J", &host.project_frame());
    assert_eq!(host.app.diff_media_page(), 2);
    assert_eq!(
        crate::ui::status::context_spans(&host.app),
        fresh_status,
        "the diff status must not name the departed Preview's page"
    );
    host.key('R');
    host.settle_inline_images("PDF Preview reopened", ROOT_WAIT);
    assert_eq!(
        host.app.page_indicator(),
        Some((1, 3)),
        "reopening Preview retains HEAD's fresh-preview page rule"
    );
}

#[test]
fn departed_preview_values_do_not_replace_diff_or_project_chrome() {
    if isolated("departed_preview_values_do_not_replace_diff_or_project_chrome") {
        return;
    }
    for (case, path) in [
        ("search", "doc.md"),
        ("outline", "doc.md"),
        ("caret", "plain.txt"),
        ("selection", "plain.txt"),
        ("zoom", "image.png"),
        ("follow", "doc.md"),
    ] {
        let root = at_home(preview_shape_repo(path));
        let text = config(
            "git",
            "en",
            "vim",
            "relative",
            "right",
            "[plugins.git]\nread_only=true\n",
        );
        let cfg: Config = toml::from_str(&text).unwrap();
        let mut host = Sim::with_config_sized(&root, cfg, 180, 14).with_media();
        host.app.tab.projects.wait_for_result();
        host.app.poll_projects();
        host.key('p');
        drain(&mut host);
        host.key('/');
        host.keys(path);
        host.enter();
        host.enter();
        drain(&mut host);
        host.settle_inline_images("fresh diff chrome", ROOT_WAIT);
        host.app.flash = None;
        let status = crate::ui::status::context_spans(&host.app);
        let footer = crate::ui::status::footer_spans(&host.app, 180);
        let help_rows = |app: &App| {
            crate::ops::help::sections(app)
                .into_iter()
                .map(|s| (s.title, s.rows))
                .collect::<Vec<_>>()
        };
        let help = help_rows(&host.app);
        host.key('R');
        if path == "doc.md" {
            host.key('R');
        }
        host.settle_inline_images("Preview chrome precondition", ROOT_WAIT);
        match case {
            "search" => {
                host.key('/');
                host.keys("Title");
                host.enter();
                assert!(host.app.preview_search_query().is_some());
            }
            "outline" => {
                host.key('O');
                assert!(host.app.is_outline());
                host.key('q');
            }
            "caret" => {
                host.key('l');
                host.key('j');
            }
            "selection" => {
                host.key('v');
                host.key('l');
                assert!(host.app.is_preview_visual());
                host.press(KeyCode::Esc, KeyModifiers::NONE);
            }
            "zoom" => host.key('+'),
            "follow" => {
                host.key('F');
                assert!(host.app.follow_enabled());
            }
            _ => unreachable!(),
        }
        host.key('R');
        host.settle_inline_images("returned diff chrome", ROOT_WAIT);
        host.app.flash = None;
        assert!(host.app.is_git_diff_preview(), "{case}: returned diff");
        assert!(
            !host.app.is_outline() && !host.app.is_preview_visual() && !host.app.is_searching(),
            "{case}: no departed modal"
        );
        if case != "follow" {
            assert_eq!(
                crate::ui::status::context_spans(&host.app),
                status,
                "{case}: diff status"
            );
            assert_eq!(
                crate::ui::status::footer_spans(&host.app, 180),
                footer,
                "{case}: diff footer"
            );
            assert_eq!(help_rows(&host.app), help, "{case}: diff help");
        } else {
            assert!(
                host.app.follow_enabled(),
                "FOLLOW is intentionally global, not a departed Preview value"
            );
        }
        host.key('p');
        drain(&mut host);
        host.app.flash = None;
        assert!(host.app.project_active());
        // Old hub: key_context() and help_context() were both Context::Screen(backend, Changes).
        // The same facts are held in the new design as the single `Canvas` key context plus the
        // canvas's own screen (`canvas_screen`), which is the changes screen of the backend; the
        // help follows that screen.
        assert_eq!(
            host.app.canvas_screen(),
            Some(crate::keymap::Context::Screen(
                crate::keymap::Backend::Git,
                crate::keymap::Screen::Changes
            )),
            "canvas screen after p"
        );
        assert_eq!(
            host.app.key_context(),
            Some(crate::keymap::Context::Canvas),
            "key context after p"
        );
        let project_status: String = crate::ui::status::context_spans(&host.app)
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        assert!(
            !project_status.contains("VISUAL")
                && !project_status.contains("x1.")
                && !project_status.contains("/3"),
            "{case}: project status"
        );
        host.key('q');
        drain(&mut host);
        assert!(host.app.is_git_diff_preview());
    }
}

#[test]
fn colocated_backend_selection_is_consistent_across_all_representations() {
    if !crate::vcs::jj::available() {
        eprintln!("NOT RUN: colocated backend parity needs real jj; 0 fake parity cases");
    }
    if !crate::vcs::jj::available() {
        return;
    }
    if isolated("colocated_backend_selection_is_consistent_across_all_representations") {
        return;
    }
    jj_colocated_representation_cases();
    report_cases();
}

#[test]
fn colocated_changed_file_navigation_uses_project_backend_in_every_representation() {
    if !crate::vcs::jj::available() {
        eprintln!("NOT RUN: 16 real colocated changed-list sessions; 0 fake cases");
    }
    if !crate::vcs::jj::available() {
        return;
    }
    if isolated("colocated_changed_file_navigation_uses_project_backend_in_every_representation") {
        return;
    }
    jj_colocated_changed_list_cases();
    report_cases();
}

fn jj_colocated_changed_list_cases() {
    use super::selected_backend::{GIT_MD, GIT_SVG, JJ_MD, JJ_SVG};
    use konoma_vcs_core::vcs::{Git, Jj, Vcs};
    for id in ["jj", "git"] {
        let names = if id == "git" {
            vec!["a.md", "b.svg", "c-git-only.md", "e-git-only.svg"]
        } else {
            vec!["a.md", "b.svg", "d-jj-only.md"]
        };
        for view in ["source", "rendered", "preview", "media"] {
            for remap in [true, false] {
                let roots = diff_repositories(
                    super::selected_backend::selected_backend_repo_with_changed_sets,
                );
                for root in [&roots.host, &roots.app] {
                    for (backend, expected) in [
                        (
                            &Git as &dyn Vcs,
                            vec!["a.md", "b.svg", "c-git-only.md", "e-git-only.svg"],
                        ),
                        (&Jj as &dyn Vcs, vec!["a.md", "b.svg", "d-jj-only.md"]),
                    ] {
                        let actual = backend
                            .changed_files(root)
                            .into_iter()
                            .map(|entry| {
                                entry.path.file_name().unwrap().to_str().unwrap().to_owned()
                            })
                            .collect::<Vec<_>>();
                        assert_eq!(
                            actual, expected,
                            "the independent colocated recipe has distinct changed sets"
                        );
                    }
                    for name in &names {
                        let backend: &dyn Vcs = if id == "git" { &Git } else { &Jj };
                        let expected = if name.ends_with(".svg") {
                            if id == "git" {
                                GIT_SVG
                            } else {
                                JJ_SVG
                            }
                        } else if id == "git" {
                            GIT_MD
                        } else {
                            JJ_MD
                        };
                        assert_eq!(
                            backend.base_contents(root, &root.join(name)).as_deref(),
                            Some(expected.as_bytes()),
                            "{id}/{name}: exact baseline blob bytes"
                        );
                    }
                }
                let file_keys = if remap {
                    "[keys.diff]\nn='jump_next_change'\nN='jump_prev_change'\n"
                } else {
                    ""
                };
                let text = config(
                    id,
                    "en",
                    "vim",
                    "relative",
                    "right",
                    &format!("[plugins.{id}]\nread_only=true\n{file_keys}"),
                )
                .replace(
                    "diff_view='source'",
                    &format!(
                        "diff_view='{}'",
                        if matches!(view, "rendered" | "media") {
                            "rendered"
                        } else {
                            "source"
                        }
                    ),
                );
                assert_eq!(
                    toml::from_str::<Config>(&text).unwrap().external.vcs,
                    "auto"
                );
                let start = usize::from(view == "media");
                let mut steps = vec![Step::Resize(180, 14), ch('/')];
                steps.extend(typed(names[start]));
                steps.extend([key(KeyCode::Enter), key(KeyCode::Enter)]);
                let mut at = std::collections::BTreeMap::new();
                at.insert(steps.len(), names[start]);
                steps.push(ch('r'));
                at.insert(steps.len(), names[start]);
                let refreshed = steps.len();
                if view == "preview" {
                    steps.extend([ch('R'), ch('R')]);
                    at.insert(steps.len(), names[start]);
                    for key in ['n', 'N', 'n', 'N'] {
                        steps.push(ch(key));
                        at.insert(steps.len(), names[start]);
                    }
                    steps.push(ch('R'));
                    at.insert(steps.len(), names[start]);
                }
                let mut cursor = start;
                for (key, dir) in [('n', 1), ('N', -1)] {
                    for _ in 0..=names.len() {
                        cursor = (cursor as isize + dir).rem_euclid(names.len() as isize) as usize;
                        steps.push(if remap {
                            ch(key)
                        } else {
                            Step::Key(KeyEvent::new(
                                KeyCode::Char(if dir > 0 { 'n' } else { 'p' }),
                                KeyModifiers::CONTROL,
                            ))
                        });
                        at.insert(steps.len(), names[cursor]);
                    }
                }
                steps.push(ch('q'));
                let mut checked = 0;
                run_roots_with(
                    (&roots.host, &roots.app),
                    id,
                    &text,
                    steps,
                    false,
                    RunOptions {
                        colocated: true,
                        pictures: true,
                        media: true,
                        launches: Some(Arc::new(Mutex::new(Vec::new()))),
                        ..Default::default()
                    },
                    |step, state, host, _| {
                        let Some(name) = at.get(&step) else {
                            return;
                        };
                        let diff = state.diff.as_ref().unwrap();
                        if step >= refreshed {
                            assert_eq!(host.app.git_vcs, crate::vcs::VcsKind::Git, "the tree keeps its global auto backend while the project remains selected");
                        }
                        assert_eq!(
                            diff.path.file_name().unwrap(),
                            *name,
                            "{id}/{view}/step {step}: selected changed-file order and wrap"
                        );
                        assert_eq!(diff.baseline, if id == "git" { "HEAD" } else { "@-" });
                        let expected = if name.ends_with(".svg") {
                            if id == "git" {
                                GIT_SVG
                            } else {
                                JJ_SVG
                            }
                        } else if id == "git" {
                            GIT_MD
                        } else {
                            JJ_MD
                        };
                        match diff.effective {
                            konoma_host_core::project::DiffView::Source => {
                                assert_eq!(
                                    diff.source_lines
                                        .iter()
                                        .filter(|(_, old, _, text)| old.is_some()
                                            && !text.contains("No newline at end of file"))
                                        .flat_map(|(_, _, _, text)| text.lines())
                                        .collect::<Vec<_>>(),
                                    expected.lines().collect::<Vec<_>>(),
                                    "{id}/{name}: Source baseline lines"
                                );
                            }
                            _ if name.ends_with(".svg") => {
                                let media = state.media.as_ref().unwrap();
                                assert!(
                                    media.cache.iter().any(|entry| entry
                                        .key
                                        .to_string_lossy()
                                        .starts_with("media-diff://o/")
                                        && entry.svg_bytes.as_deref() == Some(expected.as_bytes())),
                                    "{id}/{name}: selected media baseline bytes"
                                );
                            }
                            _ => assert_eq!(
                                diff.baseline_text.as_deref(),
                                Some(expected),
                                "{id}/{name}: Rendered/Preview baseline bytes"
                            ),
                        }
                        if diff.effective != konoma_host_core::project::DiffView::Preview {
                            let index = names
                                .iter()
                                .position(|candidate| candidate == name)
                                .unwrap();
                            assert_eq!(
                                host.app.diff_change_position(),
                                Some((index + 1, names.len())),
                                "{id}/{view}/{name}: visible selected-list position/count"
                            );
                        }
                        checked += 1;
                    },
                );
                assert_eq!(
                    checked,
                    2 + 2 * (names.len() + 1) + if view == "preview" { 6 } else { 0 }
                );
            }
        }
    }
}

#[test]
fn colocated_tree_changed_file_navigation_keeps_its_global_backend() {
    if !crate::vcs::jj::available() {
        eprintln!("NOT RUN: 2 real colocated tree cases; 0 fake cases");
    }
    if !crate::vcs::jj::available() {
        return;
    }
    if isolated("colocated_tree_changed_file_navigation_keeps_its_global_backend") {
        return;
    }
    for (backend, names) in [
        (
            "auto",
            vec!["a.md", "b.svg", "c-git-only.md", "e-git-only.svg"],
        ),
        ("jj", vec!["a.md", "b.svg", "d-jj-only.md"]),
    ] {
        let root = at_home(super::selected_backend::selected_backend_repo_with_changed_sets());
        let mut cfg = Config::default();
        cfg.project.vcs = Some(backend.into());
        let mut host = Sim::with_config_sized(&root, cfg, 180, 14);
        host.select("a.md");
        let mut cursor = 0;
        for (key, dir) in [('n', 1), ('N', -1)] {
            for _ in 0..=names.len() {
                cursor = (cursor as isize + dir).rem_euclid(names.len() as isize) as usize;
                host.key(key);
                assert!(!host.app.project_active() && !host.app.is_git_diff_preview());
                assert_eq!(
                    host.app.tab.entries[host.app.tab.selected]
                        .path
                        .file_name()
                        .unwrap(),
                    names[cursor],
                    "{backend}: the ordinary tree uses its own changed list"
                );
            }
        }
    }
}

fn jj_colocated_representation_cases() {
    jj_colocated_tool_cases();
    jj_colocated_text_gutters();
    for id in ["git", "jj"] {
        for path in ["a.md", "b.svg"] {
            let roots = diff_repositories(super::selected_backend::selected_backend_repo);
            let text = config(
                id,
                "en",
                "vim",
                "relative",
                "right",
                &format!("[plugins.{id}]\nread_only=true\n"),
            );
            let cfg: Config = toml::from_str(&text).unwrap();
            assert_eq!(cfg.external.vcs, "auto", "the global default remains auto");
            let expected = if path == "a.md" {
                if id == "git" {
                    "# Git HEAD\n"
                } else {
                    "# Jj parent\n"
                }
            } else if id == "git" {
                "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"1\" height=\"1\"><!-- Git HEAD --></svg>"
            } else {
                "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"1\" height=\"1\"><!-- Jj parent --></svg>"
            };
            let mut steps = vec![Step::Resize(180, 14), ch('/')];
            steps.extend(typed(path));
            steps.extend([
                key(KeyCode::Enter),
                key(KeyCode::Enter),
                ch('!'),
                ch('p'),
                ch('q'),
                ch('r'),
                ch('R'),
                ch('!'),
                ch('p'),
                ch('q'),
                ch('R'),
                ch('R'),
                ch('!'),
                ch('p'),
                ch('q'),
                ch('R'),
                ch('q'),
            ]);
            let mut seen = std::collections::BTreeSet::new();
            run_roots_with(
                (&roots.host, &roots.app),
                id,
                &text,
                steps,
                false,
                RunOptions {
                    colocated: true,
                    pictures: true,
                    media: true,
                    launches: Some(Arc::new(Mutex::new(Vec::new()))),
                    ..Default::default()
                },
                |_, state, _, _| {
                    if let Some(diff) = &state.diff {
                        assert_eq!(diff.baseline, if id == "git" { "HEAD" } else { "@-" });
                        seen.insert(format!("{:?}", diff.effective));
                        if diff.effective == konoma_host_core::project::DiffView::Source {
                            let missing_newline =
                                diff.source_lines.iter().any(|(_, old, _, line)| {
                                    old.is_some() && line == "\n\\ No newline at end of file"
                                });
                            let mut bytes: Vec<u8> = diff
                                .source_lines
                                .iter()
                                .filter(|(_, old, _, line)| {
                                    old.is_some() && line != "\n\\ No newline at end of file"
                                })
                                .flat_map(|(_, _, _, line)| {
                                    line.bytes().chain(std::iter::once(b'\n'))
                                })
                                .collect();
                            if missing_newline {
                                assert_eq!(bytes.pop(), Some(b'\n'));
                            }
                            // jj's hunk transport omits the missing-newline marker. Compare its
                            // displayed lines here; exact revision bytes are checked independently
                            // below and in the Rendered/Preview/media worker outcomes.
                            assert_eq!(
                                String::from_utf8(bytes)
                                    .unwrap()
                                    .lines()
                                    .collect::<Vec<_>>(),
                                expected.lines().collect::<Vec<_>>(),
                                "{id}/{path}: Source baseline lines"
                            );
                        } else if path == "a.md" {
                            assert_eq!(
                                diff.baseline_text.as_deref(),
                                Some(expected),
                                "{id}: Rendered/Preview baseline bytes"
                            );
                        } else if diff.effective == konoma_host_core::project::DiffView::Rendered {
                            let media = state.media.as_ref().unwrap();
                            let old: Vec<_> = media
                                .cache
                                .iter()
                                .filter(|c| c.key.to_string_lossy().starts_with("media-diff://o/"))
                                .collect();
                            assert!(!old.is_empty(), "media baseline cache exists");
                            for entry in old {
                                assert_eq!(
                                    entry.svg_bytes.as_deref(),
                                    Some(expected.as_bytes()),
                                    "{id}: media baseline bytes"
                                );
                            }
                            assert!(media.held.as_ref().unwrap().contains(if id == "git" {
                                "base: Head"
                            } else {
                                "base: JjParent"
                            }));
                        }
                    }
                },
            );
            assert_eq!(
                seen,
                ["Source", "Rendered", "Preview"]
                    .into_iter()
                    .map(str::to_owned)
                    .collect()
            );
        }
    }
}

#[test]
fn colocated_diff_tool_uses_the_selected_backend() {
    if !crate::vcs::jj::available() {
        return;
    }
    if isolated("colocated_diff_tool_uses_the_selected_backend") {
        return;
    }
    jj_colocated_tool_cases();
}

fn jj_colocated_tool_cases() {
    for id in ["git", "jj"] {
        let roots = diff_repositories(super::selected_backend::selected_backend_repo);
        let text = config(
            id,
            "en",
            "vim",
            "relative",
            "right",
            &format!("[plugins.{id}]\nread_only=true\n"),
        );
        let mut steps = vec![Step::Resize(180, 14), ch('/')];
        steps.extend(typed("a.md"));
        steps.extend([
            key(KeyCode::Enter),
            key(KeyCode::Enter),
            ch('r'),
            ch('!'),
            ch('q'),
        ]);
        run_roots_with(
            (&roots.host, &roots.app),
            id,
            &text,
            steps,
            false,
            RunOptions {
                colocated: true,
                media: true,
                launches: Some(Arc::new(Mutex::new(Vec::new()))),
                ..Default::default()
            },
            |_, _, _, _| {},
        );
    }
}

fn jj_colocated_text_gutters() {
    for id in ["git", "jj"] {
        for path in ["c.rs", "d.txt"] {
            for gutter in [true, false] {
                let roots =
                    diff_repositories(super::selected_backend::selected_backend_repo_with_code);
                let text = config(
                    id,
                    "en",
                    "vim",
                    "relative",
                    "right",
                    &format!("[plugins.{id}]\nread_only=true\n"),
                )
                .replace(
                    "diff_view='source'",
                    &format!("diff_view='source'\ngit_gutter={gutter}\nsyntax_highlight=false"),
                );
                let mut steps = vec![Step::Resize(180, 14), ch('/')];
                steps.extend(typed(path));
                steps.extend([
                    key(KeyCode::Enter),
                    key(KeyCode::Enter),
                    ch('R'),
                    ch('p'),
                    ch('q'),
                    ch('R'),
                    ch('q'),
                ]);
                let mut saw_preview = false;
                run_roots_with(
                    (&roots.host, &roots.app),
                    id,
                    &text,
                    steps,
                    false,
                    RunOptions {
                        colocated: true,
                        launches: Some(Arc::new(Mutex::new(Vec::new()))),
                        ..Default::default()
                    },
                    |_, state, _, _| {
                        if let Some(diff) = &state.diff {
                            assert_eq!(diff.baseline, if id == "git" { "HEAD" } else { "@-" });
                            if diff.effective == konoma_host_core::project::DiffView::Source {
                                let expected = if id == "git" {
                                    "// Git HEAD\nlet stable = 1;\n// Working file\n"
                                } else {
                                    "// Working file\nlet stable = 1;\n// Jj parent\n"
                                };
                                assert_eq!(
                                    diff.old_bytes(),
                                    expected.as_bytes(),
                                    "{id}/{path}: exact Source baseline bytes"
                                );
                            }
                            if diff.effective == konoma_host_core::project::DiffView::Preview {
                                saw_preview = true;
                                assert_eq!(diff.first_change, gutter.then_some(if id=="git" {0} else {2}), "{id}/{path}/gutter={gutter}: selected baseline's first changed line");
                            }
                        }
                    },
                );
                assert!(saw_preview);
            }
        }
    }
}

fn assert_colocated_repositories(host: &Path, app: &Path) {
    let contents = |root: &Path| {
        let mut bytes = Vec::new();
        for path in ["a.md", "b.svg"] {
            bytes.push(std::fs::read(root.join(path)).unwrap());
            for (program, args) in [
                ("git", vec!["show".into(), format!("HEAD:{path}")]),
                ("git", vec!["show".into(), format!(":{path}")]),
                (
                    "jj",
                    vec![
                        "file".into(),
                        "show".into(),
                        "-r".into(),
                        "@-".into(),
                        path.into(),
                    ],
                ),
                (
                    "jj",
                    vec![
                        "file".into(),
                        "show".into(),
                        "-r".into(),
                        "@".into(),
                        path.into(),
                    ],
                ),
            ] {
                let mut command = std::process::Command::new(program);
                if program == "jj" {
                    command.arg("--ignore-working-copy");
                }
                let output = command.args(args).current_dir(root).output().unwrap();
                assert!(
                    output.status.success(),
                    "colocated baseline read: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
                bytes.push(output.stdout);
            }
        }
        bytes
    };
    assert_eq!(
        contents(host),
        contents(app),
        "independent colocated repository contents, index, HEAD, @ and @-"
    );
}

#[test]
fn wrapped_preview_hides_horizontal_moves_in_footer_help_and_menu() {
    if isolated("wrapped_preview_hides_horizontal_moves_in_footer_help_and_menu") {
        return;
    }
    use crate::keymap::{Action, LeaderId, Motion};
    let mut failures = Vec::new();
    for path in ["doc.md", "plain.txt", "code.rs"] {
        for wrap in [true, false] {
            for width in [48, 180] {
                let root = at_home(preview_shape_repo(path));
                std::fs::write(root.join(path), "a long line ".repeat(200)).unwrap();
                let mut cfg: Config =
                    toml::from_str(&config("git", "en", "vim", "relative", "right", "")).unwrap();
                cfg.ui.wrap = wrap;
                for context in ["text", "document"] {
                    let table = cfg.keys.surfaces.entry(context.into()).or_default();
                    for (key, action) in [
                        ("h", "left"),
                        ("l", "right"),
                        ("0", "line_home"),
                        ("$", "line_end"),
                    ] {
                        table.insert(format!(", {key}"), format!("navigate:{action}"));
                    }
                }
                let mut host = Sim::with_config_sized(&root, cfg, width, 14).with_media();
                host.select(path);
                host.enter();
                host.settle_inline_images(
                    "wrapped plain preview",
                    std::time::Duration::from_secs(30),
                );
                let ctx = crate::ops::help::help_context(&host.app);
                let enabled = host.app.is_windowed() || !wrap;
                for (key, moves) in [('l', true), ('h', false), ('$', true), ('0', false)] {
                    host.key(key);
                    host.app.preview_enter_visual(false);
                    let caret_moved = matches!(host.app.preview_selection(), crate::app::PreviewSelection::Char { end: (_, col), .. } if col > 0);
                    host.app.preview_exit_visual();
                    assert_eq!(
                        if host.app.is_windowed() {
                            caret_moved
                        } else {
                            host.app.tab.preview_hscroll > 0
                        },
                        enabled && moves,
                        "{path}/wrap={wrap}/{width}: {key} position"
                    );
                }
                let sections = crate::ops::help::sections_for(&host.app, ctx);
                let rows: Vec<_> = sections.iter().flat_map(|s| &s.rows).collect();
                let horizontal = rows.iter().any(|(keys, _)| keys.contains("h / l"));
                let menu = crate::ops::menu::menu(
                    &host.app,
                    ctx,
                    LeaderId::Options,
                    crate::ops::menu::View::Open,
                )
                .unwrap();
                let menu_horizontal = menu
                    .items
                    .iter()
                    .any(|i| i.action == Action::Navigate(Motion::Right));
                let footer = crate::ops::footer::tokens(&host.app, ctx).join("  ");
                let footer_horizontal = footer.contains("h/l:hscroll");
                let fixture = read_snapshot_for_test("vcs_app_media");
                let kind = match path {
                    "doc.md" => "plain_document",
                    "plain.txt" => "text",
                    _ => "code",
                };
                assert_eq!(
                    footer,
                    fixture[kind].as_str().unwrap(),
                    "HEAD footer plan: {path}/{wrap}/{width}"
                );
                eprintln!("Preview {path}, wrap={wrap}, width={width}, windowed={}, acts={enabled}, help={horizontal}, menu={menu_horizontal}, footer={footer}", host.app.is_windowed());
                if horizontal != enabled
                    || menu_horizontal != enabled
                    || (!enabled && footer_horizontal)
                {
                    dump_parity_frame("plain Preview body and footer", &host.project_frame());
                    host.key('?');
                    dump_parity_frame("plain Preview help", &host.project_frame());
                    failures.push(format!("{path}/wrap={wrap}/{width}: help={horizontal}, menu={menu_horizontal}, footer={footer_horizontal}, acts={enabled}"));
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn wrapped_diff_hides_horizontal_moves_in_footer_help_and_menu() {
    if isolated("wrapped_diff_hides_horizontal_moves_in_footer_help_and_menu") {
        return;
    }
    wrapped_diff_host_cases("git");
    if !crate::vcs::jj::available() {
        eprintln!("NOT RUN: wrapped diff: 20 real jj cases; 0 fake contract cases");
    }
    if !crate::vcs::jj::available() {
        return;
    }
    jj_wrapped_diff_host_cases();
}

fn jj_wrapped_diff_host_cases() {
    wrapped_diff_host_cases("jj");
}

fn wrapped_diff_host_cases(id: &str) {
    use crate::keymap::{Action, Context, LeaderId, Motion};
    let horizontal = [
        Motion::Left,
        Motion::Right,
        Motion::LineHome,
        Motion::LineEnd,
    ];
    let mut runs = 0;
    for surface in [
        "rendered",
        "source-unified",
        "source-split",
        "diff-preview",
        "plain-preview",
    ] {
        for wrap in [true, false] {
            for width in [48, 180] {
                let root = at_home(if id == "git" {
                    preview_shape_repo("doc.md")
                } else {
                    jj_repo(false)
                });
                let base = format!("# Title\n\n{}\n", "base long line ".repeat(100));
                let changed = format!("# Title\n\n{}\n", "changed long line ".repeat(100));
                std::fs::write(root.join("doc.md"), base).unwrap();
                if id == "git" {
                    crate::test_support::run_git(&root, &["add", "doc.md"]);
                    crate::test_support::run_git(&root, &["commit", "-qm", "wrapped diff base"]);
                } else {
                    jj(&root, &["describe", "-m", "wrapped diff base"]);
                    jj(&root, &["new", "-m", "wrapped diff work"]);
                }
                std::fs::write(root.join("doc.md"), changed).unwrap();
                let text = config(
                    id,
                    "en",
                    "vim",
                    "relative",
                    "right",
                    &format!("[plugins.{id}]\nread_only=true\n"),
                );
                let mut cfg: Config = toml::from_str(&text).unwrap();
                cfg.ui.wrap = wrap;
                cfg.ui.diff_view = if surface == "rendered" {
                    "rendered"
                } else {
                    "source"
                }
                .into();
                cfg.git.diff = if surface == "source-split" {
                    "split"
                } else {
                    "unified"
                }
                .into();
                for context in ["diff", "document"] {
                    let table = cfg.keys.surfaces.entry(context.into()).or_default();
                    for (key, action) in [
                        ("h", "left"),
                        ("l", "right"),
                        ("0", "line_home"),
                        ("$", "line_end"),
                    ] {
                        table.insert(format!(", {key}"), format!("navigate:{action}"));
                    }
                }
                let mut host = Sim::with_config_sized(&root, cfg, width, 14).with_media();
                host.app.tab.projects.wait_for_result();
                host.app.poll_projects();
                if surface == "plain-preview" {
                    host.select("doc.md");
                    host.enter();
                } else {
                    host.key('p');
                    drain(&mut host);
                    host.key('/');
                    host.keys("doc.md");
                    host.enter();
                    host.enter();
                    drain(&mut host);
                    if surface == "diff-preview" {
                        host.key('R');
                        host.key('R');
                    }
                }
                host.settle_inline_images("wrapped diff host", std::time::Duration::from_secs(30));
                let ctx = crate::ops::help::help_context(&host.app);
                // Source has no wrapped body, independently of the global preview setting.
                let enabled = surface.starts_with("source") || !wrap;
                eprintln!("RUN: wrapped diff: {id}, {surface}, wrap={wrap}, width={width}");
                if ctx == Context::Diff {
                    for motion in horizontal {
                        assert_eq!(
                            crate::ops::gate::action_enabled(
                                &host.app,
                                ctx,
                                Action::Navigate(motion)
                            ),
                            enabled,
                            "{surface}: gate for {motion:?}"
                        );
                    }
                }
                let footer = crate::ops::footer::tokens(&host.app, ctx).join("  ");
                let sections = crate::ops::help::sections_for(&host.app, ctx);
                let rows = sections.iter().flat_map(|s| &s.rows).collect::<Vec<_>>();
                for (key, moves) in [('l', true), ('h', false), ('$', true), ('0', false)] {
                    host.key(key);
                    assert_eq!(
                        host.app.tab.preview_hscroll > 0,
                        enabled && moves,
                        "{id}/{surface}/wrap={wrap}/{width}: {key} horizontal state"
                    );
                    eprintln!(
                        "{id}/{surface}/wrap={wrap}/{width}: {key}: x={}",
                        host.app.tab.preview_hscroll
                    );
                }
                if !enabled && footer.contains("h/l:hscroll") {
                    dump_parity_frame(
                        "horizontal keys act: body and footer",
                        &host.project_frame(),
                    );
                    host.key('?');
                    dump_parity_frame("horizontal keys act: help", &host.project_frame());
                    host.key('q');
                }
                assert!(
                    enabled || !footer.contains("h/l:hscroll"),
                    "{surface}: inert horizontal footer: {footer}"
                );
                if ctx == Context::Document {
                    let fixture = read_snapshot_for_test("vcs_app_media");
                    let kind = if surface == "diff-preview" {
                        "diff_document"
                    } else {
                        "plain_document"
                    };
                    assert_eq!(
                        footer,
                        fixture[kind].as_str().unwrap(),
                        "HEAD Document footer plan: {id}/{surface}/{wrap}/{width}"
                    );
                } else {
                    let fixture = read_snapshot_for_test("vcs_app_media");
                    let kind = if surface.starts_with("source") {
                        "source"
                    } else if wrap {
                        "rendered_wrap"
                    } else {
                        "rendered_nowrap"
                    };
                    assert_eq!(
                        footer,
                        fixture[kind].as_str().unwrap(),
                        "HEAD diff footer plan: {id}/{surface}/{wrap}/{width}"
                    );
                }
                eprintln!("HEAD footer {id}/{surface}/{wrap}/{width}: {footer}");
                if rows.iter().any(|(keys, _)| keys.contains("h / l")) != enabled {
                    dump_parity_frame("wrapped preview body and footer", &host.project_frame());
                    host.key('?');
                    dump_parity_frame("wrapped preview help", &host.project_frame());
                    host.key('q');
                }
                assert_eq!(
                    rows.iter().any(|(keys, _)| keys.contains("h / l")),
                    enabled,
                    "{surface}: horizontal help: {rows:?}"
                );
                let menu = crate::ops::menu::menu(
                    &host.app,
                    ctx,
                    LeaderId::Options,
                    crate::ops::menu::View::Open,
                )
                .unwrap();
                for motion in horizontal {
                    assert_eq!(
                        menu.items
                            .iter()
                            .any(|i| i.action == Action::Navigate(motion)),
                        enabled,
                        "{surface}: horizontal menu {motion:?}"
                    );
                }
                host.key('?');
                let lines = crate::ui::help::help_lines(&host.app).len();
                let max_scroll = konoma_widgets::help_panel::max_scroll(
                    lines,
                    host.term.backend().buffer().area,
                );
                let mut seen_horizontal = false;
                for _ in 0..=max_scroll {
                    let help = host.project_frame();
                    seen_horizontal |= (0..help.area.height).any(|y| {
                        (0..help.area.width)
                            .map(|x| help[(x, y)].symbol())
                            .collect::<String>()
                            .contains("h / l")
                    });
                    host.key('j');
                }
                assert_eq!(
                    seen_horizontal, enabled,
                    "{surface}: drawn help over every page"
                );
                runs += 1;
            }
        }
    }
    eprintln!("wrapped diff: {runs} real {id} cases; 0 fake contract cases");
}

fn host_media_snapshot(
    host: &Sim,
    state: &ProjectStateSnapshot,
    paints: Vec<(String, konoma_widgets::picture_overlay::PicturePaintRequest)>,
) -> konoma_plugin::app::project::media_test_hooks::MediaStateSnapshot {
    use konoma_plugin::app::project::media_test_hooks::MediaStateSnapshot;
    let mut snapshot = if state.diff.is_some() || state.return_to_preview {
        let worker = host.app.media_diff_for_test();
        MediaStateSnapshot::worker(
            worker,
            host.app.tab.preview_path.as_ref().unwrap(),
            host.app.diff_media_page(),
            host.app.media_diff_layout(),
            host.app.picker_cell_px(),
            (worker.busy() || worker.queued().is_some())
                && crate::poll_timeout(&host.app) > std::time::Duration::ZERO,
        )
    } else {
        MediaStateSnapshot::default()
    };
    snapshot.cache = host.app.media_cache_for_test();
    snapshot.paints = paints;
    snapshot
}

#[test]
fn diff_screen_help_and_project_keys_match_host_in_every_representation() {
    if isolated("diff_screen_help_and_project_keys_match_host_in_every_representation") {
        return;
    }
    diff_chrome_cases("git");
    if !crate::vcs::jj::available() {
        eprintln!("NOT RUN: diff chrome real jj cases; 0 fake parity cases");
    }
    if !crate::vcs::jj::available() {
        return;
    }
    jj_diff_chrome_cases();
    report_cases();
}

fn jj_diff_chrome_cases() {
    diff_chrome_cases("jj");
}

fn diff_chrome_cases(id: &str) {
    for lang in ["en", "ja"] {
        for width in [48, 180] {
            for (view, layout, wrap) in [
                ("source", "unified", true),
                ("source", "split", false),
                ("rendered", "unified", true),
            ] {
                eprintln!("RUN: diff chrome: {id}, {lang}, {width}, {view}, {layout}, wrap={wrap}");
                let roots = diff_repositories(|| preview_shape_repo_backend("doc.md", id));
                let text = config(
                    id,
                    lang,
                    "vim",
                    "relative",
                    "right",
                    &format!("[plugins.{id}]\nread_only=true\n"),
                )
                .replace(
                    "diff_view='source'",
                    &format!("diff_view='{view}'\nwrap={wrap}"),
                )
                .replace("diff='unified'", &format!("diff='{layout}'"));
                let mut steps = vec![Step::Resize(width, 14), ch('/')];
                steps.extend(typed("doc.md"));
                steps.extend([
                    key(KeyCode::Enter),
                    key(KeyCode::Enter),
                    ch('p'),
                    ch('q'),
                    ch('!'),
                    ch('r'),
                    ch('y'),
                    ch('n'),
                ]);
                if view == "rendered" && wrap {
                    steps.extend([ch('h'), ch('l'), ch('0'), ch('$')]);
                }
                steps.push(ch('?'));
                steps.push(Step::HelpPages);
                steps.extend([
                    ch('G'),
                    ch('g'),
                    ch('q'),
                    ch(','),
                    key(KeyCode::Esc),
                    ch(' '),
                    key(KeyCode::Esc),
                    ch('q'),
                ]);
                let mut coverage = HelpCoverage::default();
                run_roots_with(
                    (&roots.host, &roots.app),
                    id,
                    &text,
                    steps,
                    false,
                    RunOptions {
                        launches: Some(Arc::new(Mutex::new(Vec::new()))),
                        ..Default::default()
                    },
                    |_, state, host, _| {
                        coverage.observe(state, host.term.backend().buffer().area);
                        if view == "rendered" && wrap {
                            if let Some(diff) = state.diff.as_ref() {
                                assert_eq!(
                                    diff.hscroll, 0,
                                    "wrapped Rendered clamps every horizontal key"
                                );
                            }
                        }
                    },
                );
                coverage.complete();
            }
        }
    }
}

#[test]
fn media_both_sides_pages_keys_and_previews_match_host_git_en() {
    if isolated("media_both_sides_pages_keys_and_previews_match_host_git_en") {
        return;
    }
    media_project_cases("git", "en");
    report_cases();
}

#[test]
fn media_both_sides_pages_keys_and_previews_match_host_git_ja() {
    if isolated("media_both_sides_pages_keys_and_previews_match_host_git_ja") {
        return;
    }
    media_project_cases("git", "ja");
    report_cases();
}

#[test]
fn media_both_sides_pages_keys_and_previews_match_host_jj_en() {
    if !crate::vcs::jj::available() {
        eprintln!("NOT RUN: U8g media real jj cases; 0 fake parity cases");
        return;
    }
    if isolated("media_both_sides_pages_keys_and_previews_match_host_jj_en") {
        return;
    }
    jj_media_project_cases("en");
    report_cases();
}

#[test]
fn media_both_sides_pages_keys_and_previews_match_host_jj_ja() {
    if !crate::vcs::jj::available() {
        eprintln!("NOT RUN: U8g media real jj cases; 0 fake parity cases");
        return;
    }
    if isolated("media_both_sides_pages_keys_and_previews_match_host_jj_ja") {
        return;
    }
    jj_media_project_cases("ja");
    report_cases();
}

fn jj_media_project_cases(lang: &str) {
    media_project_cases("jj", lang);
}

#[test]
fn jj_deleted_media_keeps_the_same_keys_after_project_and_refresh() {
    if !crate::vcs::jj::available() {
        return;
    }
    if isolated("jj_deleted_media_keeps_the_same_keys_after_project_and_refresh") {
        return;
    }
    let roots = diff_repositories(|| preview_shape_repo_backend("deleted.png", "jj"));
    let text = config(
        "jj",
        "en",
        "vim",
        "relative",
        "right",
        "[plugins.jj]\nread_only=true\n",
    );
    let mut steps = vec![Step::Resize(180, 14), ch('/')];
    steps.extend(typed("deleted.png"));
    steps.extend([
        key(KeyCode::Enter),
        key(KeyCode::Enter),
        ch('p'),
        ch('q'),
        ch('!'),
        ch('r'),
        ch('s'),
        ch('s'),
        ch('?'),
        ch('G'),
        ch('g'),
        ch('?'),
        ch('R'),
        ch('R'),
        ch('q'),
    ]);
    run_roots_with(
        (&roots.host, &roots.app),
        "jj",
        &text,
        steps,
        false,
        RunOptions {
            pictures: true,
            media: true,
            launches: Some(Arc::new(Mutex::new(Vec::new()))),
            ..Default::default()
        },
        |_, _, _, _| {},
    );
}

#[test]
fn diff_help_uses_drawn_surface_for_media_text_and_summaries() {
    if isolated("diff_help_uses_drawn_surface_for_media_text_and_summaries") {
        return;
    }
    for path in [
        "image.svg",
        "image.png",
        "binary.bin",
        "added.png",
        "deleted.png",
        "broken.png",
    ] {
        for pictures in [true, false] {
            let roots = diff_repositories(|| preview_shape_repo(path));
            let text = config(
                "git",
                "en",
                "vim",
                "relative",
                "right",
                "[plugins.git]\nread_only=true\n",
            );
            let mut steps = vec![Step::Resize(180, 14), ch('/')];
            steps.extend(typed(path));
            steps.extend([
                key(KeyCode::Enter),
                key(KeyCode::Enter),
                ch('l'),
                ch('?'),
                ch('G'),
                ch('g'),
                ch('q'),
                ch(','),
                key(KeyCode::Esc),
                ch('q'),
            ]);
            run_roots_with(
                (&roots.host, &roots.app),
                "git",
                &text,
                steps,
                false,
                RunOptions {
                    pictures,
                    media: true,
                    launches: Some(Arc::new(Mutex::new(Vec::new()))),
                    ..Default::default()
                },
                |_, state, host, _| {
                    if let Some(help) = state.diff.as_ref().and_then(|d| d.help.as_ref()) {
                        let acts = crate::ops::gate::action_enabled(
                            &host.app,
                            crate::keymap::Context::Diff,
                            crate::keymap::Action::Navigate(crate::keymap::Motion::Right),
                        );
                        let named = help
                            .sections
                            .iter()
                            .flat_map(|(_, rows)| rows)
                            .any(|(keys, _)| keys.contains("h / l"));
                        assert_eq!(named,acts,"{path}/pictures={pictures}: full help follows the drawn surface and acting keys");
                    }
                },
            );
        }
    }
}

#[test]
fn media_pages_cache_and_natural_size_match_host_at_each_transition() {
    if isolated("media_pages_cache_and_natural_size_match_host_at_each_transition") {
        return;
    }
    for path in ["image.png", "pages.pdf", "image.svg", "crop.png"] {
        let roots = diff_repositories(|| preview_shape_repo(path));
        let old = std::process::Command::new("git")
            .args(["show", &format!("HEAD:{path}")])
            .current_dir(&roots.host)
            .output()
            .unwrap();
        assert!(old.status.success());
        let old_hash = konoma_widgets::media_diff::fnv1a64(&old.stdout);
        let new_hash =
            konoma_widgets::media_diff::fnv1a64(&std::fs::read(roots.host.join(path)).unwrap());
        let mut saw_page_three = false;
        let text = config(
            "git",
            "en",
            "vim",
            "relative",
            "right",
            "[plugins.git]\nread_only=true\n",
        );
        let mut steps = vec![Step::Resize(180, 14), ch('/')];
        steps.extend(typed(path));
        steps.extend([key(KeyCode::Enter), key(KeyCode::Enter)]);
        if path == "image.svg" {
            steps.extend([ch('l'), ch('?'), ch('q'), ch('R')]);
        }
        steps.extend([ch('J'), ch('J'), ch('K'), ch('s'), ch('s'), ch('r')]);
        if path == "image.svg" {
            steps.push(ch('R'));
        }
        steps.extend([ch('R'), ch('R'), ch('q')]);
        run_roots_with(
            (&roots.host, &roots.app),
            "git",
            &text,
            steps,
            false,
            RunOptions {
                pictures: true,
                media: true,
                launches: Some(Arc::new(Mutex::new(Vec::new()))),
                ..Default::default()
            },
            |_, state, _, _| {
                use konoma_widgets::media_diff::MediaSideView;
                if let Some(media) = state.media.as_ref().filter(|m| m.sides.is_some()) {
                    let (old, new) = media.sides.as_ref().unwrap();
                    if path == "pages.pdf" && media.page == Some(3) {
                        saw_page_three = true;
                        assert!(
                            matches!(old, MediaSideView::PageMissing),
                            "page 3 must be missing on the 2-page side"
                        );
                    }
                    for (side, picture, hash, natural, count) in [
                        (
                            'o',
                            old,
                            old_hash,
                            match path {
                                "pages.pdf" => (40, 60),
                                "image.svg" => (40, 20),
                                "crop.png" => (320, 60),
                                _ => (8, 8),
                            },
                            Some(2),
                        ),
                        (
                            'n',
                            new,
                            new_hash,
                            match path {
                                "pages.pdf" => (80, 30),
                                "image.svg" => (20, 40),
                                "crop.png" => (80, 240),
                                _ => (8, 8),
                            },
                            Some(3),
                        ),
                    ] {
                        if let MediaSideView::Picture {
                            key,
                            natural_px,
                            page_count,
                            ..
                        } = picture
                        {
                            assert_eq!(
                                *natural_px, natural,
                                "{path}/{side}: natural dimensions independent of decoded raster"
                            );
                            assert_eq!(*page_count, if path == "pages.pdf" { count } else { None });
                            let prefix =
                                format!("media-diff://{side}/{hash:016x}/{}", media.page.unwrap());
                            assert!(key==&prefix || key.starts_with(&(prefix+"/")),"{path}/{side}: cache key must retain side, raw content hash and page: {key}");
                        }
                    }
                }
            },
        );
        if path == "pages.pdf" {
            assert!(
                saw_page_three,
                "both panes must reach page 3, beyond the shorter side"
            );
        }
    }
}

#[test]
fn diff_refresh_reloads_working_bytes_in_source_rendered_and_media() {
    if isolated("diff_refresh_reloads_working_bytes_in_source_rendered_and_media") {
        return;
    }
    for (path, view) in [
        ("doc.md", "source"),
        ("doc.md", "rendered"),
        ("image.png", "source"),
    ] {
        let roots = diff_repositories(|| preview_shape_repo(path));
        let text = config(
            "git",
            "en",
            "vim",
            "relative",
            "right",
            "[plugins.git]\nread_only=true\n",
        )
        .replace("diff_view='source'", &format!("diff_view='{view}'"));
        let mut steps = vec![Step::Resize(180, 14), ch('/')];
        steps.extend(typed(path));
        steps.extend([key(KeyCode::Enter), key(KeyCode::Enter)]);
        steps.push(Step::BeforeKey(
            Box::new(move |root| {
                if path == "image.png" {
                    write_solid_png(&root.join(path), 16, 12, [100, 200, 30]);
                } else {
                    std::fs::write(root.join(path), "# AFTER REFRESH\n\nchanged.\n").unwrap();
                }
            }),
            KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE),
        ));
        steps.push(ch('q'));
        run_roots_with(
            (&roots.host, &roots.app),
            "git",
            &text,
            steps,
            false,
            RunOptions {
                pictures: true,
                media: true,
                launches: Some(Arc::new(Mutex::new(Vec::new()))),
                ..Default::default()
            },
            |_, _, _, _| {},
        );
    }
}

fn media_project_cases(id: &str, lang: &str) {
    let widths = [48, 180];
    let paths = [
        "image.png",
        "image.svg",
        "image.gif",
        "pages.pdf",
        "added.png",
        "deleted.png",
        "broken.png",
        "crop.png",
    ];
    let mut ran = 0usize;
    {
        for width in widths {
            for path in paths {
                eprintln!("RUN: media {id}/{lang}/{width}/{path}");
                let roots = diff_repositories(|| preview_shape_repo_backend(path, id));
                let text = config(
                    id,
                    lang,
                    "vim",
                    "relative",
                    "right",
                    &format!("[plugins.{id}]\nread_only=true\n"),
                );
                let mut steps = vec![Step::Resize(width, 14), ch('/')];
                steps.extend(typed(path));
                steps.extend([key(KeyCode::Enter), key(KeyCode::Enter)]);
                if path == "image.svg" {
                    steps.extend([ch('l'), ch('?')]);
                    steps.push(Step::HelpPages);
                    steps.extend([ch('q'), ch('R')]);
                }
                if path == "pages.pdf" {
                    steps.extend([
                        ch('J'),
                        ch('J'),
                        ch('J'),
                        ch('K'),
                        key(KeyCode::PageDown),
                        key(KeyCode::PageUp),
                    ]);
                }
                steps.extend([ch('p'), ch('q'), ch('!'), ch('r')]);
                if path == "image.svg" {
                    steps.push(ch('R'));
                }
                steps.extend([ch('s'), ch('s'), ch('?')]);
                steps.push(Step::HelpPages);
                steps.extend([
                    key(KeyCode::PageDown),
                    key(KeyCode::PageUp),
                    ch('?'),
                    ch(','),
                    key(KeyCode::Esc),
                    ch(' '),
                    key(KeyCode::Esc),
                    Step::Resize(if width == 48 { 180 } else { 48 }, 14),
                    ch('R'),
                    ch('+'),
                    ch('='),
                    ch('R'),
                    ch('q'),
                ]);
                let mut coverage = HelpCoverage::default();
                run_roots_with(
                    (&roots.host, &roots.app),
                    id,
                    &text,
                    steps,
                    false,
                    RunOptions {
                        pictures: true,
                        media: true,
                        launches: Some(Arc::new(Mutex::new(Vec::new()))),
                        ..Default::default()
                    },
                    |_, state, host, _| {
                        coverage.observe(state, host.term.backend().buffer().area);
                        if state.diff.as_ref().is_some_and(|diff| {
                            diff.effective == konoma_host_core::project::DiffView::Rendered
                        }) {
                            let media = state.media.as_ref().unwrap();
                            assert_eq!(host.app.diff_media_page(), media.page.unwrap());
                            if path == "pages.pdf" && media.page == Some(3) {
                                assert!(
                                    matches!(
                                        media.sides,
                                        Some((
                                            konoma_widgets::media_diff::MediaSideView::PageMissing,
                                            _
                                        ))
                                    ),
                                    "the shorter side must retain PageMissing at page 3"
                                );
                            }
                        }
                    },
                );
                coverage.complete();
                ran += 1;
            }
        }
    }
    assert_eq!(
        ran,
        widths.len() * paths.len(),
        "every width x path session of media {id}/{lang} must run"
    );
    assert!(ran >= 16, "media {id}/{lang} ran only {ran} sessions");
}
