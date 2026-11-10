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
