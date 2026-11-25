//! Literal geometry and cells pinned before extracting the inline overlay.
use super::*;
use crate::config::Config;
use ratatui::{
    backend::TestBackend,
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    Terminal,
};

fn setup() -> (
    App,
    crate::test_support::TmpDir,
    std::sync::mpsc::Receiver<MdEncodeRequest>,
) {
    let dir = crate::test_support::unique_tmp("inline_overlay_reference");
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("doc.md");
    std::fs::write(&file, "plain").unwrap();
    let mut app = App::new(dir.to_path_buf(), Config::default()).unwrap();
    app.picker = Some(ratatui_image::picker::Picker::halfblocks());
    app.enter_preview(&file);
    app.ensure_md_cache(16);
    let cache = app.md_cache.as_mut().unwrap();
    cache.lines = vec![Line::from("wrapped upstream"), Line::from("image")];
    cache.row_prefix = vec![0, 2, 5];
    cache.images = vec![crate::preview::markdown::ImagePlacement {
        url: "mermaid-fence://one".into(),
        alt: String::new(),
        line: 1,
        col: 4,
        cols: 4,
        rows: 3,
        fence_ord: Some(3),
    }];
    cache.diff_marks = vec![(0..1, crate::preview::markdown::DiffMark::Added)];
    app.md_images.cache.insert(
        PathBuf::from("mermaid-fence://one"),
        MdImgEntry {
            decoded: Some(Arc::new(image::DynamicImage::ImageRgba8(
                image::RgbaImage::from_pixel(8, 6, image::Rgba([10, 20, 30, 255])),
            ))),
            ..Default::default()
        },
    );
    let (tx, rx) = std::sync::mpsc::channel();
    app.attach_md_encoder(tx);
    (app, dir, rx)
}

fn draw(app: &mut App) -> Buffer {
    let mut term = Terminal::new(TestBackend::new(20, 10)).unwrap();
    app.begin_md_overlay_frame();
    let b = term
        .draw(|f| {
            for c in &mut f.buffer_mut().content {
                c.set_symbol("x").set_style(
                    Style::new()
                        .fg(Color::Red)
                        .bg(Color::Blue)
                        .add_modifier(Modifier::BOLD),
                );
            }
            crate::ui::preview::overlay_inline_images(f, app, Rect::new(2, 1, 16, 7));
        })
        .unwrap()
        .buffer
        .clone();
    app.finish_md_overlay_frame();
    b
}

fn reference(actual: &Buffer, target: Rect) {
    assert_eq!(actual.area, Rect::new(0, 0, 20, 10));
    assert!(target.width > 0 && target.height > 0);
    let mut count = 0;
    let mut nonempty = 0;
    for y in 0..10 {
        for x in 0..20 {
            let c = &actual[(x, y)];
            let reset = target.contains((x, y).into());
            assert_eq!(
                (c.symbol(), c.fg, c.bg, c.modifier),
                (
                    "x",
                    if reset { Color::Reset } else { Color::Red },
                    Color::Blue,
                    if reset {
                        Modifier::empty()
                    } else {
                        Modifier::BOLD
                    }
                ),
                "{x},{y}"
            );
            count += 1;
            nonempty += usize::from(c.symbol() != " ");
        }
    }
    assert_eq!(count, 200);
    assert_eq!(nonempty, 200);
}

#[test]
fn inline_overlay_cells_and_requests_match_both_adapters() {
    let (mut app, _dir, rx) = setup();
    app.tab.preview_scroll = 3;
    reference(&draw(&mut app), Rect::new(7, 1, 4, 2));
    let req = rx.recv_timeout(std::time::Duration::from_secs(20)).unwrap();
    assert_eq!(req.path, PathBuf::from("mermaid-fence://one"));
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
}

#[test]
fn inline_overlay_signature_matches_the_pre_move_literal() {
    let (mut app, _dir, _) = setup();
    draw(&mut app);
    // Captured by running 50c379cc's overlay_inline_images on this fixture, replacing only
    // note_md_overlay with the capture assertion. The original adapter, not the shared builder,
    // produced this literal (inner 2,1,16,7; gutter 1; visual row 2; no focus; zoom 1).
    assert_eq!(
        app.md_overlay.last_signature_for_test(),
        Some(12_555_909_121_415_213_079u64)
    );
}

#[test]
fn ready_inline_overlay_signatures_match_both_adapters_and_pre_move_literal() {
    use konoma_plugin::app::{DocPics, InProcessSvg, Laid};
    use konoma_widgets::inline_images::InlineImage;

    let (mut app, _dir, host_requests) = setup();
    let inner = Rect::new(2, 1, 16, 7);
    let cache = app.md_cache.as_ref().unwrap();
    let laid = Laid {
        lines: cache.lines.clone(),
        row_prefix: cache.row_prefix.clone(),
        images: cache.images.clone(),
        max_line_cols: cache.max_line_cols,
    };
    assert_eq!(laid.images.len(), 1);
    assert_eq!(app.md_visual_span(laid.images[0].line).0, 2);
    assert_eq!(laid.first_row_of(laid.images[0].line), 2);
    assert!(app.md_gutter_active());
    assert_eq!(app.focused_mermaid_ordinal(), None);
    assert_eq!(app.fence_zoom_level(), 1.0);
    assert_eq!(app.tab.preview_scroll, 0);

    let path = PathBuf::from(&laid.images[0].url);
    let pixels = app.md_images.cache[&path].decoded.as_ref().unwrap().clone();
    let ready = || {
        MdProtoSlot::encoded(
            MdEncodeKey::Full { cols: 4, rows: 3 },
            InlineImage::Kitty(konoma_widgets::kitty::KittyImage::build(
                &pixels.to_rgba8(),
                4,
                3,
                42,
                false,
            )),
        )
    };
    app.md_images.cache.get_mut(&path).unwrap().full = vec![ready()];
    let mut doc = DocPics::new(
        &ratatui_image::picker::Picker::halfblocks(),
        "test",
        &path,
        800,
        Arc::new(InProcessSvg),
    );
    let (sdk_tx, sdk_requests) = std::sync::mpsc::channel();
    doc.cache_for_test().enc_tx = Some(sdk_tx);
    doc.cache_for_test().cache.insert(
        path.clone(),
        MdImgEntry {
            decoded: Some(pixels.clone()),
            full: vec![ready()],
            ..Default::default()
        },
    );
    assert!(app.md_images.proto(&path, 4, 3, 0, 3).is_some());
    assert!(doc.cache_for_test().proto(&path, 4, 3, 0, 3).is_some());

    let host = draw(&mut app);
    let mut sdk = Buffer::empty(host.area);
    for cell in &mut sdk.content {
        cell.set_symbol("x").set_style(
            Style::new()
                .fg(Color::Red)
                .bg(Color::Blue)
                .add_modifier(Modifier::BOLD),
        );
    }
    doc.draw_with(&mut sdk, inner, Some((&laid, 0)), 1, None);
    let host_sig = app.md_overlay.last_signature_for_test().unwrap();
    let sdk_sig = doc.overlay_for_test().last_signature_for_test().unwrap();
    // This is konoma's 50c379cc literal, captured before extraction, not a value generated by
    // DocPics or by the shared renderer. Both adapters have every protocol ready and the same
    // URL, target (7,3,4,3), inner, gutter, visual rows, scroll, focus=None and zoom=1.
    assert_eq!(host_sig, sdk_sig);
    assert_eq!(host_sig, 12_555_909_121_415_213_079u64);
    assert_eq!(sdk_sig, 12_555_909_121_415_213_079u64);
    assert!(host_requests.try_recv().is_err());
    assert!(sdk_requests.try_recv().is_err());
    assert_eq!(host.area, Rect::new(0, 0, 20, 10));
    assert_eq!(sdk.area, host.area);
    let mut compared = 0;
    let mut nonempty = 0;
    for y in 0..10 {
        for x in 0..20 {
            let (a, b) = (&host[(x, y)], &sdk[(x, y)]);
            assert_eq!(
                (a.symbol(), a.fg, a.bg, a.modifier),
                (b.symbol(), b.fg, b.bg, b.modifier),
                "{x},{y}"
            );
            compared += 1;
            nonempty += usize::from(a.symbol() != " ");
        }
    }
    assert_eq!(compared, 200);
    assert!(nonempty > 0);
    assert!(host[(7, 3)].symbol().contains('\u{10eeee}'));
}

#[test]
fn pending_visible_reservation_does_not_change_signature_when_encoded() {
    let (mut app, _dir, rx) = setup();
    reference(&draw(&mut app), Rect::new(7, 3, 4, 3));
    assert!(!app.take_md_overlay_moved());
    let req = rx.recv_timeout(std::time::Duration::from_secs(20)).unwrap();
    let image = konoma_widgets::inline_images::InlineImage::Kitty(
        konoma_widgets::kitty::KittyImage::build(&req.image.to_rgba8(), 4, 3, 42, false),
    );
    app.md_images
        .apply_encode(konoma_widgets::inline_images::MdEncodeResult {
            path: req.path,
            key: req.key,
            image: Some(image),
            born: req.born,
        });
    let ready = draw(&mut app);
    assert!(ready[(7, 3)]
        .symbol()
        .starts_with("\x1b_Gq=2,i=42,a=T,U=1,f=32,o=z,t=d,s=8,v=6,m=0;eNrjEpH7zzWEMQD/qzsR\x1b\\"));
    assert!(ready
        .content
        .iter()
        .any(|c| c.symbol().contains('\u{10eeee}')));
    assert!(!app.take_md_overlay_moved());
}

#[test]
fn overlay_signature_tracks_geometry_focus_zoom_and_departure() {
    let (mut app, _dir, _) = setup();
    draw(&mut app);
    assert!(!app.take_md_overlay_moved());
    app.md_cache.as_mut().unwrap().images[0].url = "mermaid-fence://two".into();
    draw(&mut app);
    assert!(app.take_md_overlay_moved());
    draw(&mut app);
    assert!(!app.take_md_overlay_moved());
    app.tab.fence_zoom = 2.0;
    draw(&mut app);
    assert!(app.take_md_overlay_moved());
    app.md_items = vec![MdItem {
        kind: MdItemKind::MermaidFence { ordinal: 3 },
        line: 0,
    }];
    app.tab.focused_item = Some(0);
    draw(&mut app);
    assert!(app.take_md_overlay_moved());
    app.tab.preview_scroll = 20;
    draw(&mut app);
    assert!(app.take_md_overlay_moved());
    app.tab.preview_scroll = 21;
    draw(&mut app);
    assert!(!app.take_md_overlay_moved());
}

#[test]
fn overlay_move_clears_once_and_worker_results_redraw_without_input() {
    let (mut app, _dir, rx) = setup();
    for (scroll, moved) in [
        (0, false),
        (0, false),
        (1, true),
        (1, false),
        (20, true),
        (21, false),
    ] {
        app.tab.preview_scroll = scroll;
        draw(&mut app);
        assert_eq!(app.take_md_overlay_moved(), moved, "scroll {scroll}");
    }
    assert_eq!(
        crate::poll_timeout(&app),
        std::time::Duration::from_millis(16)
    );
    let request = rx.recv_timeout(std::time::Duration::from_secs(20)).unwrap();
    let (jobs, work) = std::sync::mpsc::channel();
    let (done, results) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        konoma_widgets::inline_images::md_encode_worker(
            ratatui_image::picker::Picker::halfblocks(),
            work,
            done,
        )
    });
    jobs.send(request).unwrap();
    drop(jobs);
    let result = results
        .recv_timeout(std::time::Duration::from_secs(20))
        .unwrap();
    worker.join().unwrap();
    assert!(
        app.apply_md_encode(result),
        "a completed encode requests redraw"
    );
    app.tab.preview_scroll = 0;
    let b = draw(&mut app);
    assert!(b.content.iter().any(|c| c.fg == Color::Rgb(10, 20, 30)));
    let e = app
        .md_images
        .cache
        .get_mut(&PathBuf::from("mermaid-fence://one"))
        .unwrap();
    let image = e.decoded.as_ref().unwrap().clone();
    e.frames = vec![
        (image.clone(), std::time::Duration::from_secs(600)),
        (image, std::time::Duration::from_secs(600)),
    ];
    e.shown_at = Some(std::time::Instant::now() - std::time::Duration::from_secs(1200));
    assert!(app.md_gif_poll_timeout().is_some());
    assert!(
        app.advance_md_gifs_if_due(),
        "a GIF deadline requests redraw"
    );
    assert_eq!(
        app.md_images.cache[&PathBuf::from("mermaid-fence://one")].idx,
        1
    );
}

#[test]
fn focused_fence_uses_source_ordinal_and_full_visibility() {
    let (mut app, _dir, rx) = setup();
    app.md_items = vec![MdItem {
        kind: MdItemKind::MermaidFence { ordinal: 0 },
        line: 0,
    }];
    app.tab.focused_item = Some(0);
    reference(&draw(&mut app), Rect::new(7, 3, 4, 3));
    let _ = rx.recv_timeout(std::time::Duration::from_secs(20)).unwrap();
    app.md_items[0].kind = MdItemKind::MermaidFence { ordinal: 3 };
    let focused = draw(&mut app);
    let rows = [
        "xxxxxxxxxxxxxxxxxxxx",
        "xxxxxxxxxxxxxxxxxxxx",
        "xx┌ ◇ mermaid — E┐xx",
        "xx│xxxxxxxxxxxxxx│xx",
        "xx│xxxxxxxxxxxxxx│xx",
        "xx│xxxxxxxxxxxxxx│xx",
        "xx└──────────────┘xx",
        "xxxxxxxxxxxxxxxxxxxx",
        "xxxxxxxxxxxxxxxxxxxx",
        "xxxxxxxxxxxxxxxxxxxx",
    ];
    assert_eq!(focused.area, Rect::new(0, 0, 20, 10));
    let mut count = 0;
    for (y, row) in rows.iter().enumerate() {
        for (x, symbol) in row.chars().enumerate() {
            let border = (y == 2 || y == 6) && (2..18).contains(&x)
                || (3..6).contains(&y) && (x == 2 || x == 17);
            let picture = (3..6).contains(&y) && (7..11).contains(&x);
            let title = y == 2 && (3..17).contains(&x);
            let c = &focused[(x as u16, y as u16)];
            assert_eq!(
                (c.symbol(), c.fg, c.bg, c.modifier),
                (
                    symbol.to_string().as_str(),
                    if border {
                        Color::Cyan
                    } else if picture {
                        Color::Reset
                    } else {
                        Color::Red
                    },
                    Color::Blue,
                    if title {
                        Modifier::BOLD | Modifier::REVERSED
                    } else if picture {
                        Modifier::empty()
                    } else {
                        Modifier::BOLD
                    }
                ),
                "{x},{y}"
            );
            count += 1;
        }
    }
    assert_eq!(count, 200);
    assert!(focused.content.iter().any(|c| c.symbol() == "┌"));
    app.tab.preview_scroll = 3;
    app.tab.fence_zoom = 2.0;
    reference(&draw(&mut app), Rect::new(7, 1, 4, 2));
}
