use konoma_plugin::app::diff_screen::test_hooks::DiffScreenTestHooks;
use std::sync::{mpsc, Arc};
use std::time::Duration;

use konoma_plugin::app::{AppEvent, EventSource, Next};
use konoma_plugin::host::project::media_diff::*;
use konoma_plugin::{Key, KeyCode};
use konoma_widgets::media_diff::MediaDiffLayout;

mod departure;
mod fixture;
mod support;
mod wake;

fn computed(req: &MediaDiffRequest) -> MediaDiffComputed {
    let side = |old: bool| {
        let count = if old { 2 } else { 4 };
        if req.page > count {
            return MediaDiffSideDecoded::PageMissing;
        }
        MediaDiffSideDecoded::Picture(Box::new(MediaDiffPictureDecoded {
            natural_px: if old { (40, 20) } else { (20, 40) },
            bytes: 8,
            page_count: Some(count),
            image: image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
                2,
                1,
                image::Rgba([10, 20, 30, 255]),
            )),
            frames: None,
            svg: None,
            svg_base: None,
            cache_key: format!(
                "media-diff://{}/{}/{}x{}",
                if old { "o" } else { "n" },
                req.page,
                req.raster_px.0,
                req.raster_px.1
            )
            .into(),
        }))
    };
    MediaDiffComputed::Ready {
        kind: MediaDiffKind::Pdf,
        base: MediaBase::Head,
        same_bytes: false,
        old: side(true),
        new: side(false),
    }
}

#[test]
fn media_s_j_k_sequence_matches_host_cells_and_picture_requests() {
    let dir = konoma_test_support::unique_tmp("media_screen_sequence");
    let mut prepared = support::prepared(&dir);
    prepared.diff = Some(prepared.diff.take().unwrap().with_media_worker(
        MediaDiffWorker::with_compute(Arc::new(computed)),
        mpsc::channel().1,
    ));
    struct Keys(std::vec::IntoIter<Key>);
    impl EventSource for Keys {
        fn next(&mut self, _: Option<Duration>) -> Next {
            self.0
                .next()
                .map_or(Next::End, |key| Next::Event(AppEvent::Key(key)))
        }
    }
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(54, 14)).unwrap();
    konoma_plugin::app::project::run_project_with(
        &mut terminal,
        &mut prepared,
        &mut Keys(
            vec![
                Key::char('s'),
                Key::char('s'),
                Key::char('J'),
                Key::char('J'),
                Key::char('J'),
                Key::char('J'),
                Key::char('K'),
                Key::new(KeyCode::PageDown),
            ]
            .into_iter(),
        ),
        &Default::default(),
        None,
    )
    .unwrap();
    let state = prepared.diff.as_ref().unwrap();
    assert_eq!(
        state.media_page(),
        4,
        "both sides share the longer side's page clamp"
    );
    assert_eq!(
        state.media_layout(),
        MediaDiffLayout::Stack,
        "s cycles the media layout"
    );
    fixture::check_screens();
    fixture::check_real_pdf_pages();
}

#[test]
fn diff_hints_help_and_options_follow_real_execution() {
    use konoma_host_core::{
        keymap::{KeyPress, Motion},
        project::{DiffLayout, DiffView},
        shared::SharedConfig,
    };
    use konoma_plugin::app::diff_screen::DiffScreenState;
    use konoma_plugin::host::project::diff_presentation::{DiffHelpSection, DiffUiOp};
    use konoma_plugin::host::project::md_diff::{MdDiffOutcome, MdDiffWorker};
    let dir = konoma_test_support::unique_tmp("media_screen_hints");
    let prepared = support::prepared(&dir);
    let state = prepared.diff.as_ref().unwrap();
    let p = state.presentation();
    assert!(
        !p.enabled(DiffUiOp::Layout),
        "pending media has no layout key"
    );
    assert!(!p.enabled(DiffUiOp::Discard));
    assert!(!p.enabled(DiffUiOp::Project));
    assert!(!p.enabled(DiffUiOp::Follow));
    assert!(p
        .help_rows_with(&[])
        .iter()
        .all(|r| r.section != DiffHelpSection::Tabs));
    assert_eq!(
        state
            .options_menu()
            .iter()
            .map(|i| i.action)
            .collect::<Vec<_>>(),
        vec![DiffUiOp::PathStyle],
        "only executable options survive in declared order"
    );
    let cfg =
        SharedConfig::parse("[ui]\nlang='en'\n[keys.diff]\n', a'='cycle_diff_layout'\n").unwrap();
    let mut request = state.request.clone();
    request.path = dir.join("doc.md");
    let markdown = || {
        MdDiffWorker::with_compute(Arc::new(|_| MdDiffOutcome::Ready {
            old_pre: "old\n".into(),
            new_pre: "new\n".into(),
            ops: konoma_widgets::markdown::diff_align("old\n", "new\n").2,
            any_change: true,
            front_matter_only: false,
        }))
    };
    let mut source = DiffScreenState::open_with_markdown_worker(
        request.clone(),
        prepared.services.clone(),
        &cfg,
        DiffLayout::Unified,
        Some(DiffView::Source),
        markdown(),
        mpsc::channel().1,
    )
    .unwrap();
    assert_eq!(
        source
            .options_menu()
            .iter()
            .map(|i| i.key)
            .collect::<Vec<_>>(),
        vec![KeyPress::ch('p'), KeyPress::ch('a')],
        "the resolved declaration keeps p before the appended a"
    );
    source.handle_key(KeyPress::ch(',')).unwrap();
    source.handle_key(KeyPress::ch('a')).unwrap();
    assert_eq!(
        source.layout,
        DiffLayout::Split,
        "the options item executes the advertised operation"
    );
    let empty_cfg = SharedConfig::parse("[ui]\nlang='en'\n[keys.diff]\n', p'='noop'\n").unwrap();
    let empty = DiffScreenState::open(
        request.clone(),
        prepared.services.clone(),
        &empty_cfg,
        DiffLayout::Unified,
        Some(DiffView::Source),
    )
    .unwrap();
    assert!(empty.options_menu().is_empty());
    assert!(
        !empty.presentation().enabled(DiffUiOp::Options),
        "an empty options menu has no trigger hint"
    );
    for wrap in [true, false] {
        let mut cfg = cfg.clone();
        cfg.ui.wrap = wrap;
        let mut rendered = DiffScreenState::open_with_markdown_worker(
            request.clone(),
            prepared.services.clone(),
            &cfg,
            DiffLayout::Unified,
            Some(DiffView::Rendered),
            markdown(),
            mpsc::channel().1,
        )
        .unwrap();
        rendered.poll().unwrap();
        rendered.prepare_frame(ratatui::layout::Rect::new(0, 1, 54, 12), None);
        let p = rendered.presentation();
        assert!(
            !p.enabled(DiffUiOp::Layout),
            "Rendered Markdown has no s action"
        );
        assert_eq!(
            p.enabled(DiffUiOp::Navigate(Motion::Right)),
            !wrap,
            "horizontal movement is advertised only when it acts"
        );
        assert!(
            p.footer_tokens()
                .iter()
                .all(|t| !wrap || !t.ops.contains(&DiffUiOp::Navigate(Motion::Right))),
            "wrapping has no horizontal footer hint"
        );
        assert!(
            p.help_rows_with(&[])
                .iter()
                .all(|r| !wrap || !r.ops.contains(&DiffUiOp::Navigate(Motion::Right))),
            "wrapping has no horizontal help hint"
        );
        assert!(rendered.help_sections().iter().all(|s| s.title != "Tabs"));
    }
}

#[test]
fn media_worker_encode_and_gif_wake_without_input() {
    wake::check();
}

#[test]
fn media_eviction_rekicks_and_overlay_departure_clears_once() {
    departure::check();
}
