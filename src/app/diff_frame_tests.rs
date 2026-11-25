//! Cell references captured from the host before extracting the diff frame.
use super::*;
use crate::config::Config;
use konoma_plugin::host::project::diff_frame as frame_api;
use ratatui::{backend::TestBackend, buffer::Buffer, layout::Rect, style::Modifier, Terminal};
use serde_json::{json, Value};

fn setup() -> (App, crate::test_support::TmpDir) {
    let dir = crate::test_support::unique_tmp("diff_frame_reference");
    let root = dir.join("reference");
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("doc.md");
    std::fs::write(
        &path,
        "# Heading\n\n  alpha beta gamma delta epsilon zeta eta theta iota kappa lambda\n\nlast\n",
    )
    .unwrap();
    let mut cfg = Config::default();
    cfg.ui.lang = "en".into();
    let mut app = App::new(root, cfg).unwrap();
    app.path_style = PathStyle::Relative;
    app.enter_preview(&path);
    (app, dir)
}

fn draw(app: &mut App, width: u16, height: u16) -> Buffer {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal
        .draw(|f| crate::ui::preview::render(f, app, f.area()))
        .unwrap()
        .buffer
        .clone()
}

fn reference(name: &str, actual: &Buffer) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("snapshots/diff_frame.json");
    let records: Value =
        serde_json::from_str(&std::fs::read_to_string(path).expect("pre-move fixture")).unwrap();
    let expected = &records["cases"][name];
    let expected_cells: Vec<_> = expected["runs"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|run| {
            std::iter::repeat_n(
                &records["palette"][run[1].as_u64().unwrap() as usize],
                run[0].as_u64().unwrap() as usize,
            )
        })
        .collect();
    assert_eq!(
        expected["rect"],
        json!([0, 0, actual.area.width, actual.area.height])
    );
    assert_eq!(
        actual.area,
        Rect::new(0, 0, actual.area.width, actual.area.height)
    );
    let mut compared = 0;
    for (c, e) in actual.content.iter().zip(&expected_cells) {
        assert_eq!(
            (c.symbol(), c.fg, c.bg, c.modifier),
            (
                e[0].as_str().unwrap(),
                e[1].as_str().unwrap().parse().unwrap(),
                e[2].as_str().unwrap().parse().unwrap(),
                Modifier::from_bits(e[3].as_u64().unwrap() as u16).unwrap()
            ),
            "{name}: cell {compared}"
        );
        compared += 1;
    }
    assert_eq!(
        compared,
        usize::from(actual.area.width) * usize::from(actual.area.height)
    );
    assert_eq!(expected_cells.len(), compared);
    assert!(actual.content.iter().any(|c| c.symbol() != " "));
}

fn source(app: &mut App) {
    let path = app.tab.preview_path.clone().unwrap();
    app.tab.preview_kind = Some(PreviewKind::GitDiff(path.clone()));
    app.tab.diff_view = DiffView::Source;
    app.diff_cache = Some(DiffCache {
        path,
        lines: (0..60)
            .map(|i| crate::git::DiffLine {
                kind: if i % 3 == 0 {
                    crate::git::DiffLineKind::Removed
                } else if i % 3 == 1 {
                    crate::git::DiffLineKind::Added
                } else {
                    crate::git::DiffLineKind::Context
                },
                old_no: Some(i + 1),
                new_no: Some(i + 1),
                text: format!(
                    "row {i:02}: alpha beta gamma delta epsilon zeta eta theta iota kappa"
                ),
            })
            .collect(),
    });
}

#[test]
fn source_diff_cells_match_layout_and_scroll_reference() {
    for (layout, label) in [
        (DiffLayout::Unified, "unified"),
        (DiffLayout::Split, "split"),
        (DiffLayout::Auto, "auto"),
    ] {
        for width in [44, 91, 92] {
            for scroll in [0, 7, u16::MAX] {
                let mut previous = None;
                for gutter in [false, true] {
                    let (mut app, _dir) = setup();
                    source(&mut app);
                    app.diff_layout = layout;
                    app.cfg.ui.git_gutter = gutter;
                    app.tab.preview_scroll = scroll;
                    app.tab.preview_hscroll = 5;
                    konoma_widgets::gitdiff::reset_highlight_calls_for_test();
                    let b = draw(&mut app, width, 7);
                    let calls = konoma_widgets::gitdiff::highlight_calls_for_test();
                    assert!(
                        calls > 0 && calls <= 10,
                        "only the window is highlighted: {calls}"
                    );
                    assert_eq!(app.diff_split_drawn, layout.is_split(width - 2));
                    reference(&format!("source-{label}-{width}-{scroll}"), &b);
                    let mut small = Buffer::empty(b.area);
                    let update = frame_api::draw_source_diff(
                        &mut small,
                        b.area,
                        frame_api::SourceDiffValues {
                            lines: &app.diff_cache.as_ref().unwrap().lines,
                            extension: "md",
                            theme: &app.cfg.ui.theme.code_theme,
                            layout: frame_api::source_diff_layout(layout),
                            scroll,
                            hscroll: 5,
                            title: frame_api::DiffTitleValues {
                                path: Some("reference/doc.md"),
                                ..Default::default()
                            },
                            empty_text: "unused",
                            scroll_words: konoma_widgets::scroll::ScrollWords {
                                all: "All",
                                top: "Top",
                                bot: "Bot",
                            },
                        },
                    );
                    reference(&format!("source-{label}-{width}-{scroll}"), &small);
                    assert_eq!(
                        (update.scroll, update.hscroll, update.diff_split_drawn),
                        (
                            app.tab.preview_scroll,
                            app.tab.preview_hscroll,
                            app.diff_split_drawn
                        )
                    );
                    if let Some(p) = previous {
                        assert_eq!(b, p, "Markdown gutter does not affect Source");
                    }
                    previous = Some(b);
                }
            }
        }
    }
}

fn ready(app: &mut App, old: &str, new: &str, kind: MdDiffKind) {
    let old_doc = crate::preview::markdown::model::Doc::parse(old);
    let new_doc = crate::preview::markdown::model::Doc::parse(new);
    let ops = crate::preview::markdown::diff::block_ops(&old_doc, &new_doc, old, new);
    let path = app.tab.preview_path.clone().unwrap();
    let gen = *app.md_diff.generation_for_test();
    *app.md_diff.landed_for_test() = Some((
        path,
        kind,
        gen,
        MdDiffOutcome::Ready {
            old_pre: old.into(),
            new_pre: new.into(),
            ops,
            any_change: old != new,
            front_matter_only: false,
        },
    ));
    app.md_cache = None;
}

#[test]
fn rendered_diff_cells_match_wrap_gutter_and_extent_reference() {
    for wrap in [false, true] {
        for gutter in [false, true] {
            for scroll in [0, 1, 7, u16::MAX] {
                let mut previous = None;
                for layout in [DiffLayout::Unified, DiffLayout::Split] {
                    let (mut app, _dir) = setup();
                    app.cfg.ui.wrap = wrap;
                    app.cfg.ui.git_gutter = gutter;
                    app.diff_layout = layout;
                    // Ordinary decorated previews use exactly the Rendered body, with optional gutter.
                    ready(&mut app, "old\n", "# Heading\n\n  alpha beta gamma delta epsilon zeta eta theta iota kappa lambda\n\nlast\n", MdDiffKind::Gutter);
                    app.tab.preview_scroll = scroll;
                    app.tab.preview_hscroll = 6;
                    let b = draw(&mut app, 24, 7);
                    reference(&format!("rendered-{wrap}-{gutter}-{scroll}"), &b);
                    reference(
                        &format!("rendered-{wrap}-{gutter}-{scroll}"),
                        &small_decorated(&mut app, b.area, " reference/doc.md "),
                    );
                    if let Some(p) = previous {
                        assert_eq!(b, p, "line layout does not affect Rendered");
                    }
                    previous = Some(b);
                }
            }
        }
    }
    #[cfg(feature = "git")]
    rendered_picture_matrix();
    for wrap in [true, false] {
        let (mut app, _dir) = setup();
        app.cfg.ui.wrap = wrap;
        app.cfg.ui.git_gutter = false;
        app.ensure_md_cache(22);
        let cache = app.md_cache.as_mut().unwrap();
        cache.lines = vec![
            Line::from("    indented alpha beta gamma delta epsilon zeta eta theta iota"),
            Line::from("end"),
        ];
        cache.max_line_cols = 65;
        cache.row_prefix = vec![0, 4, 5];
        cache.preview_gutter_marks.clear();
        app.tab.preview_scroll = 1;
        app.tab.preview_hscroll = 5;
        reference(&format!("local-scroll-{wrap}"), &draw(&mut app, 24, 4));
    }
}

fn small_decorated(app: &mut App, area: Rect, title: &str) -> Buffer {
    let inner = ratatui::widgets::Block::bordered().inner(area);
    let (total, max_line_cols) = app.md_layout(inner.width);
    let update = frame_api::clamp_decorated_scroll(frame_api::DecoratedScrollValues {
        vertical_limit: total.saturating_sub(inner.height as usize),
        max_line_cols,
        inner_width: inner.width,
        viewport: inner.height,
        scroll: app.tab.preview_scroll,
        hscroll: app.tab.preview_hscroll,
        wrap: app.cfg.ui.wrap,
    });
    let (lines, local_scroll) = app.md_slice(update.scroll, inner.height);
    let mut buf = Buffer::empty(area);
    frame_api::draw_decorated_frame(
        &mut buf,
        area,
        frame_api::DecoratedFrameValues {
            lines,
            local_scroll,
            hscroll: update.hscroll,
            wrap: app.cfg.ui.wrap,
            extent: update.extent,
            title,
            scroll_words: konoma_widgets::scroll::ScrollWords {
                all: "All",
                top: "Top",
                bot: "Bot",
            },
        },
    );
    buf
}

#[cfg(feature = "git")]
fn rendered_picture_matrix() {
    use konoma_widgets::inline_images::InlineImage;
    for gutter in [false, true] {
        let mut previous = None;
        for layout in [DiffLayout::Unified, DiffLayout::Split] {
            let (mut app, _dir) = setup();
            let path = app.tab.preview_path.clone().unwrap();
            app.tab.preview_kind = Some(PreviewKind::GitDiff(path));
            app.tab.diff_view = DiffView::Rendered;
            app.diff_layout = layout;
            app.picker = Some(ratatui_image::picker::Picker::halfblocks());
            ready(&mut app, "old\n", "new\n", MdDiffKind::Rendered);
            app.ensure_md_cache(22);
            let cache = app.md_cache.as_mut().unwrap();
            cache.lines = (0..9)
                .map(|i| Line::from(format!("reserved {i}")))
                .collect();
            cache.row_prefix = (0..=9).collect();
            cache.diff_marks = if gutter {
                vec![(0..9, crate::preview::markdown::DiffMark::Added)]
            } else {
                vec![]
            };
            cache.images = vec![crate::preview::markdown::ImagePlacement {
                url: "math://rgba".into(),
                alt: String::new(),
                line: 1,
                col: 3,
                cols: 4,
                rows: 3,
                fence_ord: None,
            }];
            let pixels = Arc::new(image::DynamicImage::ImageRgba8(
                image::RgbaImage::from_pixel(8, 6, image::Rgba([10, 20, 30, 255])),
            ));
            let key = PathBuf::from("math://rgba");
            app.md_images.cache.insert(
                key.clone(),
                MdImgEntry {
                    decoded: Some(pixels),
                    ..Default::default()
                },
            );
            let (tx, rx) = std::sync::mpsc::channel();
            app.attach_md_encoder(tx);
            app.tab.preview_scroll = 2;
            let pending = draw(&mut app, 24, 7);
            reference(&format!("rendered-picture-pending-{gutter}"), &pending);
            let req = rx.try_recv().expect("visible crop requested synchronously");
            assert_eq!(req.path, key);
            assert_eq!(
                req.key,
                MdEncodeKey::Clip {
                    cols: 4,
                    full_rows: 3,
                    row_off: 1,
                    vis_rows: 2
                }
            );
            assert_eq!((req.cols, req.rows, req.crop), (4, 2, Some((0, 2, 8, 4))));
            assert_eq!(
                req.image.to_rgba8().into_raw(),
                [10, 20, 30, 255].repeat(48)
            );
            let mut small = small_decorated(
                &mut app,
                pending.area,
                " diff ⟨rendered⟩: reference/doc.md ",
            );
            let cache = app.md_cache.as_ref().unwrap();
            let laid = konoma_plugin::app::Laid {
                lines: cache.lines.clone(),
                row_prefix: cache.row_prefix.clone(),
                images: cache.images.clone(),
                max_line_cols: cache.max_line_cols,
            };
            let mut doc = konoma_plugin::app::DocPics::new(
                &ratatui_image::picker::Picker::halfblocks(),
                "test",
                &key,
                800,
                Arc::new(konoma_plugin::app::InProcessSvg),
            );
            doc.cache_for_test().cache.insert(
                key.clone(),
                MdImgEntry {
                    decoded: Some(req.image.clone()),
                    clip: vec![MdProtoSlot::encoded(
                        req.key,
                        InlineImage::Kitty(konoma_widgets::kitty::KittyImage::build(
                            &req.image.to_rgba8(),
                            4,
                            2,
                            42,
                            false,
                        )),
                    )],
                    ..Default::default()
                },
            );
            doc.draw_with(
                &mut small,
                Rect::new(1, 1, 22, 5),
                Some((&laid, 2)),
                u16::from(gutter),
                None,
            );
            reference(&format!("rendered-picture-ready-{gutter}"), &small);
            let encoded = InlineImage::Kitty(konoma_widgets::kitty::KittyImage::build(
                &req.image.to_rgba8(),
                4,
                2,
                42,
                false,
            ));
            app.md_images
                .apply_encode(konoma_widgets::inline_images::MdEncodeResult {
                    path: req.path,
                    key: req.key,
                    image: Some(encoded),
                    born: req.born,
                });
            let b = draw(&mut app, 24, 7);
            let target = Rect::new(4 + u16::from(gutter), 1, 4, 2);
            assert!(b[(target.x, target.y)].symbol().starts_with(
                "\x1b_Gq=2,i=42,a=T,U=1,f=32,o=z,t=d,s=8,v=6,m=0;eNrjEpH7zzWEMQD/qzsR\x1b\\"
            ));
            assert_eq!(
                b.content
                    .iter()
                    .map(|c| c.symbol().matches('\u{10eeee}').count())
                    .sum::<usize>(),
                8
            );
            for y in target.y..target.bottom() {
                assert_eq!(b[(target.x, y)].symbol().matches('\u{10eeee}').count(), 4);
            }
            assert!(rx.try_recv().is_err());
            reference(&format!("rendered-picture-ready-{gutter}"), &b);
            if let Some(p) = previous {
                assert_eq!(b, p, "line layout leaves pictures and body unchanged");
            }
            previous = Some(b);
        }
    }
}

#[cfg(feature = "git")]
#[test]
fn pending_rendered_change_scroll_survives_computing_frame() {
    let (mut app, _dir) = setup();
    let path = app.tab.preview_path.clone().unwrap();
    app.tab.preview_kind = Some(PreviewKind::GitDiff(path.clone()));
    app.tab.diff_view = DiffView::Rendered;
    *app.md_diff.pending_for_test() = Some((path.clone(), MdDiffKind::Rendered));
    app.tab.diff_scroll_pending = Some(path.clone());
    reference("computing", &draw(&mut app, 24, 7));
    assert_eq!(app.tab.diff_scroll_pending, Some(path.clone()));
    assert_eq!(app.tab.preview_scroll, 0);
    let old = (0..30)
        .map(|i| format!("paragraph {i}\n\n"))
        .collect::<String>();
    let new = old.replace(
        "paragraph 20",
        "changed paragraph 20 which wraps at the measured width",
    );
    ready(&mut app, &old, &new, MdDiffKind::Rendered);
    *app.md_diff.pending_for_test() = None;
    let b = draw(&mut app, 24, 7);
    let first = app.md_first_diff_mark_row().unwrap();
    assert!(first > 7);
    assert!(app.tab.diff_scroll_pending.is_none());
    assert!(app.tab.preview_scroll > 0);
    reference("landed", &b);
    assert_eq!(app.md_cache.as_ref().unwrap().width, 22);
}

#[test]
fn binary_summary_distinguishes_equal_rounded_sizes() {
    use konoma_plugin::host::project::media_diff::{MediaBase, MediaDiffOutcome};
    for (old, new, same, expected) in [
        (
            Some(83_250),
            Some(83_252),
            false,
            Some("binary file: 81.3 KB → 81.3 KB (+2 B)"),
        ),
        (
            Some(83_250),
            Some(83_250),
            false,
            Some("binary file: 81.3 KB → 81.3 KB (same size, content differs)"),
        ),
        (Some(1), Some(2), true, None),
    ] {
        assert_eq!(
            crate::ui::preview::media_summary_line(
                crate::i18n::Lang::En,
                &MediaDiffOutcome::Summary {
                    base: MediaBase::Head,
                    same_bytes: same,
                    old_len: old,
                    new_len: new
                }
            )
            .as_deref(),
            expected
        );
    }
    assert_eq!(
        crate::ui::preview::media_summary_line(
            crate::i18n::Lang::En,
            &MediaDiffOutcome::Unavailable
        ),
        None
    );
}
