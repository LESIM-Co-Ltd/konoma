use super::*;
use crate::host::file_preview::{FilePreviewPlan, FilePreviewSource, ResolvedFilePreview};
use crate::registry::Registry;
use konoma_host_core::project::DiffTarget;

#[test]
fn diff_header_backend_chip_matches_host_language_and_style() {
    use crate::app::project::ProjectBackend;
    use ratatui::{
        layout::Alignment,
        style::{Color, Style, Stylize},
        text::{Line, Span},
        widgets::{Paragraph, Widget},
    };
    for (backend, lang, chip, bg, diff, path) in [
        (
            ProjectBackend::Git,
            Lang::En,
            " GIT ",
            Color::Yellow,
            " DIFF ",
            "  path:rel",
        ),
        (
            ProjectBackend::Git,
            Lang::Jp,
            " Git ",
            Color::Yellow,
            " 差分 ",
            "  パス:相対",
        ),
        (
            ProjectBackend::Jj,
            Lang::En,
            " JJ ",
            Color::Magenta,
            " DIFF ",
            "  path:rel",
        ),
        (
            ProjectBackend::Jj,
            Lang::Jp,
            " jj ",
            Color::Magenta,
            " 差分 ",
            "  パス:相対",
        ),
    ] {
        let config = SharedConfig::parse("[ui]\npath_style='relative'\n").unwrap();
        let mut state = DiffScreenState::open_for_project(
            FileDiffRequest {
                path: "/fixture/file".into(),
                root: "/fixture".into(),
                info: ProjectServiceInfo {
                    workdir: "/fixture".into(),
                    backend,
                },
            },
            Arc::new(super::preview_tests::Services(std::thread::current().id())),
            &config,
            DiffLayout::Unified,
            None,
            Some(FilePreviewSource::new(
                Arc::new(Registry::new()),
                vec![],
                false,
            )),
        )
        .unwrap();
        state.set_lang(lang);
        let area = Rect::new(0, 0, 90, 1);
        let mut actual = Buffer::empty(area);
        state.draw_header(&mut actual, area);
        let mut expected = Buffer::empty(area);
        Paragraph::new(Line::from(vec![
            Span::styled(chip, Style::new().fg(Color::Black).bg(bg).bold()),
            Span::raw(" "),
            Span::styled(diff, Style::new().fg(Color::White).bg(Color::Blue).bold()),
            Span::raw(path).dim(),
        ]))
        .alignment(Alignment::Right)
        .render(area, &mut expected);
        assert_eq!(actual, expected, "exact top row: {backend:?} {lang:?}");
    }
}

#[test]
fn preview_hints_exist_exactly_when_keys_act() {
    for (target, count) in [
        (DiffTarget::Other, 1),
        (DiffTarget::Text, 2),
        (DiffTarget::Markdown, 3),
        (DiffTarget::Svg, 3),
        (DiffTarget::Picture, 2),
    ] {
        for remapped in [false, true] {
            let config = SharedConfig::parse(if remapped {
                "[ui]\ndiff_view='source'\n[keys.preview_git_diff]\nx='cycle_diff_view'\nR='noop'\n"
            } else {
                "[ui]\ndiff_view='source'\n"
            })
            .unwrap();
            let mut state = DiffScreenState::open_for_project(
                FileDiffRequest {
                    path: "/fixture/file".into(),
                    root: "/fixture".into(),
                    info: super::super::project::ProjectServiceInfo {
                        workdir: "/fixture".into(),
                        backend: super::super::project::ProjectBackend::Git,
                    },
                },
                Arc::new(super::preview_tests::Services(std::thread::current().id())),
                &config,
                DiffLayout::Unified,
                None,
                Some(FilePreviewSource::new(
                    Arc::new(Registry::new()),
                    vec![],
                    false,
                )),
            )
            .unwrap();
            state.file_previews = Some(FilePreviewSource::new(
                Arc::new(Registry::new()),
                vec![],
                false,
            ));
            state.resolved_preview = Some((
                ResolvedFilePreview {
                    plan: FilePreviewPlan::Text,
                    diff_target: target,
                    media_kind: None,
                    selected_plugin: None,
                },
                true,
            ));
            state.choose_effective();
            assert_eq!(state.representations().len(), count);
            let p = state.presentation();
            let key = KeyPress::ch(if remapped { 'x' } else { 'R' });
            let foot: Vec<_> = p
                .footer_tokens()
                .iter()
                .filter(|t| t.ops.contains(&DiffUiOp::View))
                .flat_map(|t| t.keys.clone())
                .collect();
            let help: Vec<_> = p
                .resolved_rows()
                .iter()
                .flatten()
                .filter(|r| r.ops.contains(&DiffUiOp::View))
                .flat_map(|r| r.keys.clone())
                .collect();
            assert_eq!(foot, help, "footer and help agree");
            assert_eq!(
                foot.contains(&key),
                count > 1,
                "only executable representations are offered"
            );
            if remapped {
                assert!(
                    !foot.contains(&KeyPress::ch('R')),
                    "fixed R is gone after remap"
                );
                state.handle_key(KeyPress::ch('R')).unwrap();
                assert_eq!(
                    state.choice,
                    DiffView::Source,
                    "fixed R must not execute after remap"
                );
            }
            for op in [DiffUiOp::Layout, DiffUiOp::PageNext, DiffUiOp::PagePrev] {
                let expected = if op == DiffUiOp::Layout {
                    state.effective == DiffView::Source || state.media_showing_pictures()
                } else {
                    state.media_can_page()
                };
                assert_eq!(
                    p.enabled(op),
                    expected,
                    "inactive s/J/K must not be offered"
                );
                assert_eq!(
                    p.resolved_rows()
                        .iter()
                        .flatten()
                        .any(|r| r.ops.contains(&op)),
                    p.enabled(op),
                    "inactive s/J/K are absent from help: {target:?} {op:?}"
                );
                assert!(state
                    .options_menu()
                    .iter()
                    .filter(|i| i.action == op)
                    .all(|_| p.enabled(op)));
            }
            state.handle_key(key).unwrap();
            assert_eq!(state.choice != DiffView::Source, count > 1);
            if count == 3 {
                state.resolved_preview.as_mut().unwrap().1 = false;
                state.choice = DiffView::Source;
                state.choose_effective();
                assert_eq!(
                    state.representations(),
                    vec![DiffView::Source, DiffView::Rendered]
                );
                assert!(
                    state
                        .presentation()
                        .resolved_rows()
                        .iter()
                        .flatten()
                        .any(|r| {
                            r.ops.contains(&DiffUiOp::View)
                                && r.description == "cycle diff view (source ⇄ rendered)"
                        }),
                    "a deleted file's help must not offer Preview"
                );
            }
        }
    }
    for context in [CoreContext::Text, CoreContext::Document, CoreContext::Image] {
        for remapped in [false, true] {
            let config = SharedConfig::parse(if remapped {
                "[keys.text]\nx='cycle_representation'\nR='noop'\n[keys.document]\nx='cycle_representation'\nR='noop'\n[keys.image]\nx='cycle_representation'\nR='noop'\n"
            } else { "" }).unwrap();
            let keys = CoreKeys::from_keys_config(scheme_from_str("vim"), &config.keys);
            let hint =
                super::super::project::preview::PreviewReturnHint::new(&keys, context, Lang::En);
            let key = KeyPress::ch(if remapped { 'x' } else { 'R' });
            assert!(hint.keys.contains(&key), "Preview shows the return key");
            assert_eq!(hint.keys.contains(&KeyPress::ch('R')), !remapped);
        }
    }
}

/// A formatted Markdown diff resumed after a project screen is read again, but the reservation to
/// scroll to the first change belongs to the first load: a landing consumed before the cover is
/// not made again, and the reader's scroll stays.
fn resumed_rendered_diff(with_source: bool) {
    use crate::app::project::ProjectBackend;
    use crate::host::project::md_diff::{MdDiffOutcome, MdDiffWorker};
    struct MarkdownServices;
    impl ProjectServices for MarkdownServices {
        fn inspect(
            &self,
            _: &crate::host::project::MatchedProject,
            _: &crate::app::project::ProjectSettings,
        ) -> Result<ProjectServiceInfo, Failure> {
            unreachable!()
        }
        fn file_diff(&self, _: &ProjectServiceInfo, _: &std::path::Path) -> Vec<DiffLine> {
            unreachable!()
        }
        fn changed_files(&self, _: &ProjectServiceInfo) -> Vec<konoma_vcs_core::git::ChangeEntry> {
            unreachable!()
        }
        fn base_contents(&self, _: &ProjectServiceInfo, _: &std::path::Path) -> Option<Vec<u8>> {
            Some(b"base".to_vec())
        }
        fn diff_target(&self, _: &std::path::Path, _: Option<&[u8]>) -> DiffTarget {
            DiffTarget::Markdown
        }
        fn spawn_file_diff(
            self: Arc<Self>,
            request: FileDiffRequest,
        ) -> Result<FileDiffReply, Failure> {
            let (tx, rx) = mpsc::channel();
            tx.send(Ok(FileDiffData {
                lines: Vec::new(),
                paths: vec![request.path],
            }))
            .unwrap();
            Ok(rx)
        }
    }
    let dir = konoma_test_support::unique_tmp("resume_rendered_landing");
    std::fs::create_dir_all(&dir).unwrap();
    let old = (0..25)
        .map(|i| format!("paragraph {i}\n\n"))
        .collect::<String>();
    let new = format!("{old}changed\n\n{}", "tail\n\n".repeat(20));
    // The file as the worker reads it: changed while the diff is covered.
    let current = Arc::new(std::sync::Mutex::new(new));
    let read = current.clone();
    let old_text = old.clone();
    let mut worker = MdDiffWorker::with_compute(Arc::new(move |_| {
        let new = read.lock().unwrap().clone();
        let old = old_text.clone();
        let (_, _, ops) = konoma_widgets::markdown::diff_align(&old, &new);
        MdDiffOutcome::Ready {
            old_pre: old.clone(),
            new_pre: new.clone(),
            ops,
            any_change: true,
            front_matter_only: false,
        }
    }));
    let (tx, rx) = mpsc::channel();
    let (published_tx, published_rx) = mpsc::channel();
    let (forward_tx, forward_rx) = mpsc::channel();
    worker.attach(tx);
    std::thread::spawn(move || {
        for result in rx {
            if forward_tx.send(result).is_err() {
                break;
            }
            let _ = published_tx.send(());
        }
    });
    let config = SharedConfig::parse("[ui]\nlang='en'").unwrap();
    let source = with_source.then(|| {
        struct MarkdownDocument;
        impl crate::Plugin for MarkdownDocument {
            fn manifest(&self) -> crate::Manifest {
                crate::Manifest::new("markdown-document", "0")
                    .bind(crate::Bind::files(["*.md"]))
                    .shows(crate::ViewKind::Document)
            }
            fn open(
                &self,
                _: crate::Input,
                _: &crate::OpenCx,
            ) -> Result<Box<dyn crate::Session>, Failure> {
                unreachable!()
            }
        }
        let mut registry = Registry::new();
        registry
            .register(
                Arc::new(MarkdownDocument),
                crate::registry::PluginOrigin::Standard,
            )
            .unwrap();
        FilePreviewSource::new(Arc::new(registry), Vec::new(), false)
    });
    let mut state = DiffScreenState::open_with_workers(
        FileDiffRequest {
            root: dir.to_path_buf(),
            path: dir.join("doc.md"),
            info: ProjectServiceInfo {
                workdir: dir.to_path_buf(),
                backend: ProjectBackend::Git,
            },
        },
        Arc::new(MarkdownServices),
        &config,
        DiffLayout::Unified,
        Some(DiffView::Rendered),
        (worker, forward_rx),
        // With a source the target resolves asynchronously, as in a real file app.
        (source, false),
    )
    .unwrap();
    let area = Rect::new(0, 1, 54, 12);
    let drawn = |state: &mut DiffScreenState| {
        let mut frame = Buffer::empty(area);
        state.draw(&mut frame, area);
        frame
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>()
    };
    // Polls and prepares frames until `done` holds (the replies come from workers).
    let settle =
        |state: &mut DiffScreenState, what: &str, done: &dyn Fn(&DiffScreenState) -> bool| {
            let deadline = std::time::Instant::now() + Duration::from_secs(10);
            while std::time::Instant::now() < deadline {
                state.poll().unwrap();
                state.prepare_frame(area, None);
                if done(state) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            panic!("never settled: {what}");
        };
    settle(&mut state, "the first landing", &|s| s.scroll > 20);
    let _ = &published_rx;
    // The reader scrolls, then a project screen covers the diff and is left.
    state.scroll = 3;
    state.prepare_frame(area, None);
    assert_eq!(state.scroll, 3, "the landing was consumed");
    // The file changes while covered (its first paragraphs, at the scroll the reader left).
    let revised = current
        .lock()
        .unwrap()
        .replace("paragraph 3\n", "paragraph 3 REVISED\n");
    *current.lock().unwrap() = revised;
    state.resume_project_diff(&config).unwrap();
    settle(&mut state, "the read for the return", &|s| {
        s.preview_reply.is_none() && s.rendered.is_some() && s.data.is_some()
    });
    // The answers (the target resolution, the block diff) are all in: give a mutation that lands
    // on them every chance to show.
    for _ in 0..20 {
        state.poll().unwrap();
        state.prepare_frame(area, None);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(
        state.scroll, 3,
        "the return did not land on the first change again (source: {with_source})"
    );
    let text = drawn(&mut state);
    assert!(
        text.contains("REVISED"),
        "the current content is drawn (source: {with_source}): {text}"
    );
}

#[test]
fn resumed_rendered_diff_keeps_its_scroll_and_does_not_land_again() {
    resumed_rendered_diff(false);
}

/// The same with a real file-preview source: the target resolves on a worker, and its answer
/// after the return (not only `reload`) must keep the view.
#[test]
fn resumed_rendered_diff_keeps_its_scroll_when_the_target_answers_after_the_return() {
    resumed_rendered_diff(true);
}

/// A file-preview source attached after `open` (the standalone builder `with_file_previews`): the
/// host's classification at `open` sees an unknown extension as text (only Source), the attached
/// resolver answers on its worker that a Document plugin shows it (a Markdown diff, Rendered). That
/// answer is the diff's first answer: the view lands on the first change, late in the document.
/// After the landing was consumed, a return does not land again.
#[test]
fn a_source_attached_after_open_lands_its_first_answer_on_the_first_change_once() {
    use crate::app::project::ProjectBackend;
    use crate::host::project::md_diff::{MdDiffOutcome, MdDiffWorker};
    struct UnknownExtensionServices;
    impl ProjectServices for UnknownExtensionServices {
        fn inspect(
            &self,
            _: &crate::host::project::MatchedProject,
            _: &crate::app::project::ProjectSettings,
        ) -> Result<ProjectServiceInfo, Failure> {
            unreachable!()
        }
        fn file_diff(&self, _: &ProjectServiceInfo, _: &std::path::Path) -> Vec<DiffLine> {
            unreachable!()
        }
        fn changed_files(&self, _: &ProjectServiceInfo) -> Vec<konoma_vcs_core::git::ChangeEntry> {
            unreachable!()
        }
        fn base_contents(&self, _: &ProjectServiceInfo, _: &std::path::Path) -> Option<Vec<u8>> {
            Some(b"base".to_vec())
        }
        // Without a source, an extension no built-in format knows is text.
        fn diff_target(&self, _: &std::path::Path, _: Option<&[u8]>) -> DiffTarget {
            DiffTarget::Text
        }
        fn spawn_file_diff(
            self: Arc<Self>,
            request: FileDiffRequest,
        ) -> Result<FileDiffReply, Failure> {
            let (tx, rx) = mpsc::channel();
            tx.send(Ok(FileDiffData {
                lines: Vec::new(),
                paths: vec![request.path],
            }))
            .unwrap();
            Ok(rx)
        }
    }
    struct FooDocument;
    impl crate::Plugin for FooDocument {
        fn manifest(&self) -> crate::Manifest {
            crate::Manifest::new("foo-document", "0")
                .bind(crate::Bind::files(["*.foo"]))
                .shows(crate::ViewKind::Document)
        }
        fn open(
            &self,
            _: crate::Input,
            _: &crate::OpenCx,
        ) -> Result<Box<dyn crate::Session>, Failure> {
            unreachable!()
        }
    }
    let dir = konoma_test_support::unique_tmp("attached_source_landing");
    std::fs::create_dir_all(&dir).unwrap();
    let old = (0..25)
        .map(|i| format!("paragraph {i}\n\n"))
        .collect::<String>();
    let new = format!("{old}changed\n\n{}", "tail\n\n".repeat(20));
    std::fs::write(dir.join("doc.foo"), &new).unwrap();
    // The file as the worker reads it: changed while the diff is covered.
    let current = Arc::new(std::sync::Mutex::new(new));
    let read = current.clone();
    let old_text = old.clone();
    let mut worker = MdDiffWorker::with_compute(Arc::new(move |_| {
        let new = read.lock().unwrap().clone();
        let old = old_text.clone();
        let (_, _, ops) = konoma_widgets::markdown::diff_align(&old, &new);
        MdDiffOutcome::Ready {
            old_pre: old.clone(),
            new_pre: new.clone(),
            ops,
            any_change: true,
            front_matter_only: false,
        }
    }));
    let (tx, rx) = mpsc::channel();
    worker.attach(tx);
    let config = SharedConfig::parse("[ui]\nlang='en'").unwrap();
    let state = DiffScreenState::open_with_markdown_worker(
        FileDiffRequest {
            root: dir.to_path_buf(),
            path: dir.join("doc.foo"),
            info: ProjectServiceInfo {
                workdir: dir.to_path_buf(),
                backend: ProjectBackend::Git,
            },
        },
        Arc::new(UnknownExtensionServices),
        &config,
        DiffLayout::Unified,
        Some(DiffView::Rendered),
        worker,
        rx,
    )
    .unwrap();
    // Before the source: only Source can show it.
    assert_eq!(state.effective, DiffView::Source);
    let mut registry = Registry::new();
    registry
        .register(
            Arc::new(FooDocument),
            crate::registry::PluginOrigin::Standard,
        )
        .unwrap();
    let mut state = state.with_file_previews(FilePreviewSource::new(
        Arc::new(registry),
        Vec::new(),
        false,
    ));
    let area = Rect::new(0, 1, 54, 12);
    let drawn = |state: &mut DiffScreenState| {
        let mut frame = Buffer::empty(area);
        state.draw(&mut frame, area);
        frame
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>()
    };
    let settle =
        |state: &mut DiffScreenState, what: &str, done: &dyn Fn(&DiffScreenState) -> bool| {
            let deadline = std::time::Instant::now() + Duration::from_secs(10);
            while std::time::Instant::now() < deadline {
                state.poll().unwrap();
                state.prepare_frame(area, None);
                if done(state) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            panic!("never settled: {what}");
        };
    settle(
        &mut state,
        "the resolver's answer and the block diff",
        &|s| {
            s.preview_reply.is_none()
                && s.effective == DiffView::Rendered
                && s.rendered.is_some()
                && s.data.is_some()
        },
    );
    // Every answer is in: give the landing every chance to run.
    for _ in 0..20 {
        state.poll().unwrap();
        state.prepare_frame(area, None);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(
        state.scroll > 20,
        "the first answer of the attached resolver did not land on the first change: scroll {}",
        state.scroll
    );
    assert!(
        state.pending_first_change.is_none(),
        "the reservation was not consumed"
    );
    let text = drawn(&mut state);
    assert!(text.contains("changed"), "not on the first change: {text}");
    assert!(!text.contains("paragraph 0"), "still at the top: {text}");
    // The reader scrolls, then a project screen covers the diff and is left.
    state.scroll = 3;
    state.prepare_frame(area, None);
    assert_eq!(state.scroll, 3, "the landing was consumed");
    let revised = current
        .lock()
        .unwrap()
        .replace("paragraph 3\n", "paragraph 3 REVISED\n");
    *current.lock().unwrap() = revised;
    state.resume_project_diff(&config).unwrap();
    settle(&mut state, "the read for the return", &|s| {
        s.preview_reply.is_none() && s.rendered.is_some() && s.data.is_some()
    });
    for _ in 0..20 {
        state.poll().unwrap();
        state.prepare_frame(area, None);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(state.effective, DiffView::Rendered);
    assert_eq!(
        state.scroll, 3,
        "the return's answer of the same resolver landed on the first change again"
    );
    let text = drawn(&mut state);
    assert!(
        text.contains("REVISED"),
        "the current content is drawn: {text}"
    );
}
