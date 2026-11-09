//! Frozen pre-extraction cells of DocPics, independent of the shared placement builder.
use super::*;
use konoma_widgets::inline_images::{InlineImage, MdEncodeKey, MdImgEntry, MdProtoSlot};
use konoma_widgets::markdown::ImagePlacement;
use ratatui::style::{Color, Modifier, Style};
use ratatui_image::Resize;

pub(crate) fn setup() -> (
    DocPics,
    Laid,
    Receiver<konoma_widgets::inline_images::MdEncodeRequest>,
) {
    let mut d = DocPics::new(
        &Picker::halfblocks(),
        "test",
        std::path::Path::new("/x/doc.md"),
        800,
        Arc::new(super::super::pictures::InProcessSvg),
    );
    let (tx, rx) = channel();
    d.cache.enc_tx = Some(tx);
    let image = image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
        8,
        6,
        image::Rgba([10, 20, 30, 255]),
    ));
    d.cache.cache.insert(
        PathBuf::from("mermaid-fence://one"),
        MdImgEntry {
            decoded: Some(Arc::new(image)),
            ..Default::default()
        },
    );
    let laid = Laid {
        lines: vec!["wrapped upstream".into(), "image".into()],
        row_prefix: vec![0, 2, 5],
        images: vec![ImagePlacement {
            url: "mermaid-fence://one".into(),
            alt: String::new(),
            line: 1,
            col: 4,
            cols: 4,
            rows: 3,
            fence_ord: Some(3),
        }],
        max_line_cols: 16,
    };
    (d, laid, rx)
}

fn buffer() -> Buffer {
    let mut b = Buffer::empty(Rect::new(0, 0, 20, 10));
    for c in &mut b.content {
        c.set_symbol("x").set_style(
            Style::new()
                .fg(Color::Red)
                .bg(Color::Blue)
                .add_modifier(Modifier::BOLD),
        );
    }
    b
}

pub(crate) fn relayout_ready() -> DocPics {
    let (mut d, _, _) = setup();
    let path = PathBuf::from("/x/failed.png");
    d.cache.cache.insert(
        path.clone(),
        MdImgEntry {
            request: 7,
            ..Default::default()
        },
    );
    d.res_tx
        .send(DecodedImage {
            path,
            image: Err("failed".into()),
            svg: None,
            reraster: false,
            frames: None,
            request: 7,
        })
        .unwrap();
    d
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
            let picture = reset && y == target.y;
            let rgb = Color::Rgb(10, 20, 30);
            assert_eq!(
                (c.symbol(), c.fg, c.bg, c.modifier),
                (
                    if picture { " " } else { "x" },
                    if picture {
                        rgb
                    } else if reset {
                        Color::Reset
                    } else {
                        Color::Red
                    },
                    if picture { rgb } else { Color::Blue },
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
    assert_eq!(nonempty, 200 - usize::from(target.width));
}

#[test]
fn inline_overlay_cells_and_requests_match_both_adapters() {
    let (mut d, laid, rx) = setup();
    let mut b = buffer();
    d.draw(&mut b, Rect::new(2, 1, 16, 7), Some((&laid, 3)));
    let req = rx.recv_timeout(Duration::from_secs(20)).unwrap();
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
    let image = Picker::halfblocks()
        .new_protocol(
            req.image.crop_imm(0, 2, 8, 4),
            Rect::new(0, 0, 4, 2).as_size(),
            Resize::Scale(None),
        )
        .unwrap();
    let e = d.cache.cache.get_mut(&req.path).unwrap();
    e.clip = vec![MdProtoSlot::encoded(req.key, InlineImage::Proto(image))];
    e.enc_inflight = false;
    let mut b = buffer();
    d.draw(&mut b, Rect::new(2, 1, 16, 7), Some((&laid, 3)));
    reference(&b, Rect::new(6, 1, 4, 2));
    let mut b = buffer();
    d.draw_with(&mut b, Rect::new(2, 1, 16, 7), Some((&laid, 3)), 1, None);
    reference(&b, Rect::new(7, 1, 4, 2));
}

fn pending_reference(actual: &Buffer, target: Rect) {
    assert_eq!(actual.area, Rect::new(0, 0, 20, 10));
    let mut count = 0;
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
        }
    }
    assert_eq!(count, 200);
    assert!(actual.content.iter().any(|c| c.symbol() == "x"));
}

#[test]
fn pending_picture_resets_every_reserved_cell_before_a_protocol_exists() {
    let (mut d, laid, rx) = setup();
    let mut b = buffer();
    d.draw(&mut b, Rect::new(2, 1, 16, 7), Some((&laid, 3)));
    let request = rx.recv_timeout(Duration::from_secs(20)).unwrap();
    assert_eq!(
        request.key,
        MdEncodeKey::Clip {
            cols: 4,
            full_rows: 3,
            row_off: 1,
            vis_rows: 2
        }
    );
    assert!(d.cache.proto(&request.path, 4, 3, 1, 2).is_none());
    pending_reference(&b, Rect::new(6, 1, 4, 2));
}

#[test]
fn docpics_density_does_not_start_a_reraster_or_keep_the_loop_pending() {
    let (mut d, laid, rx) = setup();
    d.font = (10, 20);
    let path = PathBuf::from("mermaid-fence://one");
    d.cache.cache.get_mut(&path).unwrap().svg = Some(Arc::new(
        br#"<svg xmlns="http://www.w3.org/2000/svg" width="8" height="6"/>"#.to_vec(),
    ));
    d.draw(&mut buffer(), Rect::new(2, 1, 16, 7), Some((&laid, 0)));
    assert!(!d.cache.cache[&path].reraster_inflight);
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(20)).unwrap().key,
        MdEncodeKey::Full { cols: 4, rows: 3 }
    );
    d.cache.cache.get_mut(&path).unwrap().enc_inflight = false;
    d.cache.cache.get_mut(&path).unwrap().reraster_inflight = true;
    assert!(!d.loading());
    assert_eq!(d.wake_in(Duration::from_millis(17)), None);
}

#[test]
fn focused_fence_uses_source_ordinal_and_full_visibility() {
    let (mut d, laid, _) = setup();
    let inner = Rect::new(2, 1, 16, 7);
    let mut focus = DocPictureFocus {
        ordinal: 0,
        zoom: 1.0,
        center: (0.5, 0.5),
        image_align: konoma_widgets::markdown::BlockAlign::Center,
        title: " ◇ mermaid — Enter: full screen  +/-: zoom ",
    };
    let mut b = buffer();
    d.draw_with(&mut b, inner, Some((&laid, 0)), 1, Some(focus));
    pending_reference(&b, Rect::new(7, 3, 4, 3));
    focus.ordinal = 3;
    let mut b = buffer();
    d.draw_with(&mut b, inner, Some((&laid, 0)), 1, Some(focus));
    // Frozen from konoma before extraction, including the clipped host-translated title.
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
    assert_eq!(b.area, Rect::new(0, 0, 20, 10));
    let mut count = 0;
    for (y, row) in rows.iter().enumerate() {
        for (x, symbol) in row.chars().enumerate() {
            let border = (y == 2 || y == 6) && (2..18).contains(&x)
                || (3..6).contains(&y) && (x == 2 || x == 17);
            let picture = (3..6).contains(&y) && (7..11).contains(&x);
            let title = y == 2 && (3..17).contains(&x);
            let c = &b[(x as u16, y as u16)];
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
    assert!(b.content.iter().any(|c| c.symbol() == "┌"));
    focus.zoom = 2.0;
    let mut b = buffer();
    d.draw_with(&mut b, inner, Some((&laid, 3)), 1, Some(focus));
    pending_reference(&b, Rect::new(7, 1, 4, 2));
    let (mut d, laid, rx) = setup();
    d.draw_with(&mut buffer(), inner, Some((&laid, 0)), 1, Some(focus));
    assert!(matches!(
        rx.recv_timeout(Duration::from_secs(20)).unwrap().key,
        MdEncodeKey::Full { cols: 4, rows: 3 }
    ));
}

#[test]
fn overlay_move_clears_once_and_worker_results_redraw_without_input() {
    let (mut d, laid, rx) = setup();
    let inner = Rect::new(2, 1, 16, 7);
    for (scroll, moved) in [
        (0, false),
        (0, false),
        (1, true),
        (1, false),
        (20, true),
        (21, false),
    ] {
        d.draw(&mut buffer(), inner, Some((&laid, scroll)));
        assert_eq!(d.take_moved(), moved, "scroll {scroll}");
    }
    assert_eq!(
        d.wake_in(Duration::from_millis(17)),
        Some(Duration::from_millis(17)),
        "an encode owes the loop a wake"
    );
    let req = rx.recv_timeout(Duration::from_secs(20)).unwrap();
    let (jobs, work) = channel();
    let (done, results) = channel();
    let worker = std::thread::spawn(move || md_encode_worker(Picker::halfblocks(), work, done));
    jobs.send(req).unwrap();
    drop(jobs);
    let result = results.recv_timeout(Duration::from_secs(20)).unwrap();
    worker.join().unwrap();
    d.cache.apply_encode(result);
    assert_eq!(d.wake_in(Duration::from_millis(17)), None);
    let e = d
        .cache
        .cache
        .get_mut(&PathBuf::from("mermaid-fence://one"))
        .unwrap();
    let image = e.decoded.as_ref().unwrap().clone();
    e.frames = vec![
        (image.clone(), Duration::from_secs(600)),
        (image, Duration::from_secs(600)),
    ];
    e.shown_at = Some(std::time::Instant::now() - Duration::from_secs(1200));
    d.draw(&mut buffer(), inner, Some((&laid, 0)));
    assert!(d.wake_in(Duration::from_millis(17)).is_some());
    d.poll_at(std::time::Instant::now());
    assert_eq!(d.cache.cache[&PathBuf::from("mermaid-fence://one")].idx, 1);
    d.draw(&mut buffer(), inner, None);
    assert_eq!(
        d.wake_in(Duration::from_millis(17)),
        None,
        "a covered GIF does not wake"
    );
    let path = PathBuf::from("mermaid-fence://new");
    d.cache.cache.insert(
        path.clone(),
        MdImgEntry {
            request: 77,
            ..Default::default()
        },
    );
    d.res_tx
        .send(DecodedImage {
            path,
            image: Ok(image::DynamicImage::new_rgba8(8, 6)),
            svg: None,
            reraster: false,
            frames: None,
            request: 77,
        })
        .unwrap();
    d.poll_at(std::time::Instant::now());
    assert!(d.take_relayout());
    assert!(!d.take_relayout());
}

#[test]
fn pending_visible_reservation_does_not_change_signature_when_encoded() {
    let (mut d, laid, rx) = setup();
    d.draw(&mut buffer(), Rect::new(2, 1, 16, 7), Some((&laid, 0)));
    assert!(!d.take_moved());
    let req = rx.recv_timeout(Duration::from_secs(20)).unwrap();
    let proto = Picker::halfblocks()
        .new_protocol(
            (*req.image).clone(),
            Rect::new(0, 0, 4, 3).as_size(),
            Resize::Scale(None),
        )
        .unwrap();
    d.cache
        .apply_encode(konoma_widgets::inline_images::MdEncodeResult {
            path: req.path,
            key: req.key,
            image: Some(InlineImage::Proto(proto)),
            born: req.born,
        });
    d.draw(&mut buffer(), Rect::new(2, 1, 16, 7), Some((&laid, 0)));
    assert!(!d.take_moved());
    // A visible reservation existed before encoding, so its departure also moves the overlay.
    let path = PathBuf::from("mermaid-fence://one");
    let rgba = d.cache.cache[&path].decoded.as_ref().unwrap().to_rgba8();
    d.cache.cache.get_mut(&path).unwrap().full = vec![MdProtoSlot::encoded(
        MdEncodeKey::Full { cols: 4, rows: 3 },
        InlineImage::Kitty(konoma_widgets::kitty::KittyImage::build(
            &rgba, 4, 3, 42, false,
        )),
    )];
    let mut kitty = buffer();
    d.draw_with(
        &mut kitty,
        Rect::new(2, 1, 16, 7),
        Some((&laid, 0)),
        1,
        None,
    );
    assert!(kitty[(7, 3)]
        .symbol()
        .starts_with("\x1b_Gq=2,i=42,a=T,U=1,f=32,o=z,t=d,s=8,v=6,m=0;eNrjEpH7zzWEMQD/qzsR\x1b\\"));
    let _ = d.take_moved();
    d.draw(&mut buffer(), Rect::new(2, 1, 16, 7), None);
    assert!(d.take_moved());
    let (mut pending, laid, _) = setup();
    pending.draw(&mut buffer(), Rect::new(2, 1, 16, 7), Some((&laid, 0)));
    pending.draw(&mut buffer(), Rect::new(2, 1, 16, 7), None);
    assert!(pending.take_moved());
}

#[test]
fn overlay_signature_tracks_geometry_focus_zoom_and_departure() {
    let (mut d, mut laid, _) = setup();
    let inner = Rect::new(2, 1, 16, 7);
    d.draw(&mut buffer(), inner, Some((&laid, 0)));
    assert!(!d.take_moved());
    laid.images[0].url = "mermaid-fence://two".into();
    d.draw(&mut buffer(), inner, Some((&laid, 0)));
    assert!(d.take_moved());
    d.draw(&mut buffer(), inner, Some((&laid, 0)));
    assert!(!d.take_moved());
    d.draw_with(&mut buffer(), inner, Some((&laid, 0)), 1, None);
    assert!(d.take_moved());
    let focus = DocPictureFocus {
        ordinal: 3,
        zoom: 2.0,
        center: (0.5, 0.5),
        image_align: konoma_widgets::markdown::BlockAlign::Center,
        title: " focus ",
    };
    d.draw_with(&mut buffer(), inner, Some((&laid, 0)), 1, Some(focus));
    assert!(d.take_moved());
    let focus = DocPictureFocus { zoom: 3.0, ..focus };
    d.draw_with(&mut buffer(), inner, Some((&laid, 0)), 1, Some(focus));
    assert!(d.take_moved());
    d.draw(&mut buffer(), inner, Some((&laid, 20)));
    assert!(d.take_moved());
    d.draw(&mut buffer(), inner, Some((&laid, 21)));
    assert!(!d.take_moved());
}
