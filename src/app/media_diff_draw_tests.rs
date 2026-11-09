//! Literal references captured through the host before moving media drawing to widgets.
use super::*;
use crate::{config::Config, i18n::Lang};
use konoma_widgets::picture_overlay::{PicturePaintKind, PicturePaintRequest};
use ratatui::{backend::TestBackend, buffer::Buffer, style::Modifier, Terminal};
use serde_json::{json, Value};
use std::sync::mpsc::{channel, Receiver};
use worker::MediaDiffPictureDecoded;
use worker::{
    draw_media_diff, media_diff_draw_state, MediaDiffDrawValues, MediaDiffStateWords,
    MediaDiffWords,
};

#[derive(Clone, Copy, Debug)]
enum Sides {
    Pictures,
    Placeholders,
    Missing,
    Summary,
    Same,
    Unknown,
    Unavailable,
    Computing,
}

#[derive(Clone, Copy, Debug)]
struct Case {
    sides: Sides,
    layout: MediaDiffLayout,
    lang: Lang,
    width: u16,
    picker: bool,
}

impl Case {
    fn name(self) -> String {
        format!(
            "{:?}-{:?}-{:?}-{}-{}",
            self.sides, self.layout, self.lang, self.width, self.picker
        )
    }
}

fn cases() -> Vec<Case> {
    let mut cases = Vec::new();
    for layout in [
        MediaDiffLayout::Auto,
        MediaDiffLayout::Side,
        MediaDiffLayout::Stack,
    ] {
        for lang in [Lang::En, Lang::Jp] {
            for width in [24, 150] {
                for sides in [Sides::Pictures, Sides::Placeholders, Sides::Missing] {
                    cases.push(Case {
                        sides,
                        layout,
                        lang,
                        width,
                        picker: true,
                    });
                }
            }
        }
    }
    for lang in [Lang::En, Lang::Jp] {
        for sides in [
            Sides::Summary,
            Sides::Same,
            Sides::Unknown,
            Sides::Unavailable,
            Sides::Pictures,
            Sides::Computing,
        ] {
            cases.push(Case {
                sides,
                layout: MediaDiffLayout::Auto,
                lang,
                width: 100,
                picker: false,
            });
        }
    }
    cases
}

fn pixels() -> image::DynamicImage {
    image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
        8,
        6,
        image::Rgba([10, 20, 30, 255]),
    ))
}

fn computed(case: Case, page: u32, raster_px: (u32, u32)) -> MediaDiffComputed {
    let picture = |side, natural_px, page_count| {
        MediaDiffSideDecoded::Picture(Box::new(MediaDiffPictureDecoded {
            natural_px,
            bytes: 83_250,
            page_count: Some(page_count),
            image: pixels(),
            frames: None,
            svg: None,
            svg_base: None,
            cache_key: PathBuf::from(crate::preview::media_diff::media_diff_url(
                side,
                123,
                page,
                Some(raster_px),
            )),
        }))
    };
    let (old, new) = match case.sides {
        Sides::Pictures => (
            picture(KeySide::Old, (80, 64), 3),
            picture(KeySide::New, (40, 32), 7),
        ),
        Sides::Placeholders => (
            MediaDiffSideDecoded::Absent,
            MediaDiffSideDecoded::Failed {
                reason: "invalid PDF / 壊れた画像".into(),
            },
        ),
        Sides::Missing => (
            MediaDiffSideDecoded::PageMissing,
            MediaDiffSideDecoded::Absent,
        ),
        Sides::Unavailable => return MediaDiffComputed::Unavailable,
        Sides::Computing => unreachable!("pending worker must not compute"),
        Sides::Summary | Sides::Same | Sides::Unknown => {
            return MediaDiffComputed::Summary {
                base: MediaBase::Head,
                same_bytes: matches!(case.sides, Sides::Same),
                old_len: (!matches!(case.sides, Sides::Unknown)).then_some(83_250),
                new_len: Some(83_250),
            }
        }
    };
    MediaDiffComputed::Ready {
        kind: MediaDiffKind::Pdf,
        base: MediaBase::FollowStart,
        same_bytes: true,
        old,
        new,
    }
}

type DrawSetup = (
    App,
    crate::test_support::TmpDir,
    Receiver<MdEncodeRequest>,
    Receiver<(u32, (u32, u32))>,
);

fn setup(case: Case) -> DrawSetup {
    let dir = crate::test_support::unique_tmp("media_draw_reference");
    let root = dir.join("reference");
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("picture.pdf");
    let mut cfg = Config::default();
    cfg.ui.lang = if case.lang == Lang::En { "en" } else { "ja" }.into();
    let mut app = App::new(root, cfg).unwrap();
    app.path_style = PathStyle::Relative;
    app.enter_preview(&path);
    app.tab.preview_kind = Some(PreviewKind::GitDiff(path));
    app.tab.diff_view = DiffView::Rendered;
    app.tab.diff_media_page = 2;
    app.media_diff_layout = case.layout;
    app.picker = case.picker.then(ratatui_image::picker::Picker::halfblocks);
    let (tx, polls) = channel();
    app.media_diff = MediaDiffWorker::with_compute(Arc::new(move |req| {
        tx.send((req.page, req.raster_px)).unwrap();
        computed(case, req.page, req.raster_px)
    }));
    if matches!(case.sides, Sides::Computing) {
        *app.media_diff.busy_for_test() = true;
    }
    let (tx, encodes) = channel();
    app.attach_md_encoder(tx);
    (app, dir, encodes, polls)
}

fn draw(app: &mut App, width: u16) -> Buffer {
    let mut terminal = Terminal::new(TestBackend::new(width, 13)).unwrap();
    terminal
        .draw(|f| crate::ui::preview::render(f, app, f.area()))
        .unwrap()
        .buffer
        .clone()
}

fn fixture() -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("snapshots/media_diff.json");
    let records: Value =
        serde_json::from_str(&std::fs::read_to_string(path).expect("pre-move media fixture"))
            .unwrap();
    assert!(records["cases"].as_object().unwrap().len() >= 46);
    records
}

fn compare(records: &Value, name: &str, actual: &Buffer) {
    let expected = &records["cases"][name];
    assert_eq!(
        expected["rect"],
        json!([
            actual.area.x,
            actual.area.y,
            actual.area.width,
            actual.area.height
        ])
    );
    let cells: Vec<_> = expected["runs"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|r| {
            std::iter::repeat_n(
                &records["palette"][r[1].as_u64().unwrap() as usize],
                r[0].as_u64().unwrap() as usize,
            )
        })
        .collect();
    let mut count = 0;
    for (cell, e) in actual.content.iter().zip(&cells) {
        assert_eq!(
            (cell.symbol(), cell.fg, cell.bg, cell.modifier),
            (
                e[0].as_str().unwrap(),
                e[1].as_str().unwrap().parse().unwrap(),
                e[2].as_str().unwrap().parse().unwrap(),
                Modifier::from_bits(e[3].as_u64().unwrap() as u16).unwrap()
            ),
            "{name}: cell {count}"
        );
        count += 1;
    }
    assert_eq!(
        count,
        usize::from(actual.area.width) * usize::from(actual.area.height)
    );
    assert_eq!(count, cells.len());
    assert!(actual.content.iter().any(|c| c.symbol() != " "));
}

fn small(
    app: &App,
    area: ratatui::layout::Rect,
    painter: impl FnMut(&str, PicturePaintRequest, &mut Buffer) -> bool,
) -> Buffer {
    use crate::i18n::{tr, Msg};
    let outcome = app.media_landed_outcome_for(app.tab.preview_path.as_ref().unwrap());
    let state = media_diff_draw_state(
        outcome,
        app.picker.is_some(),
        MediaDiffStateWords {
            computing: tr(app.lang, Msg::DiffComputing),
            no_changes: tr(app.lang, Msg::GitNoChanges),
            summary: konoma_widgets::diff_frame::BinarySummaryWords {
                binary_file: tr(app.lang, Msg::MediaDiffBinaryFile),
                none: tr(app.lang, Msg::MediaDiffNone),
                same_size_different_content: tr(app.lang, Msg::MediaDiffSameSizeDifferentContent),
                byte_unit: tr(app.lang, Msg::MediaDiffByteUnit),
            },
        },
        crate::fileops::human_size,
    );
    let mut buf = Buffer::empty(area);
    draw_media_diff(
        &mut buf,
        area,
        MediaDiffDrawValues {
            state: &state,
            title: konoma_widgets::diff_frame::DiffTitleValues {
                path: Some("reference/picture.pdf"),
                representation: Some("side by side"),
                ..Default::default()
            },
            cell_px: app.picker_cell_px().unwrap_or((1, 1)),
            layout: app.media_diff_layout(),
            page: 2,
            old_label: tr(app.lang, Msg::MediaBaseFollowStart),
            new_label: tr(app.lang, Msg::MediaBaseWorkTreeGit),
            words: MediaDiffWords {
                before: tr(app.lang, Msg::MediaDiffBefore),
                after: tr(app.lang, Msg::MediaDiffAfter),
                identical: tr(app.lang, Msg::MediaDiffIdentical),
                absent_old: tr(app.lang, Msg::MediaDiffAbsentOld),
                absent_new: tr(app.lang, Msg::MediaDiffAbsentNew),
                cannot_display: tr(app.lang, Msg::MediaDiffCannotDisplay),
                page_missing: tr(app.lang, Msg::MediaDiffPageMissing),
            },
            format_size: crate::fileops::human_size,
        },
        painter,
    );
    buf
}

#[test]
fn media_diff_cells_match_side_stack_and_auto_reference() {
    let records = fixture();
    let mut count = 0;
    for case in cases() {
        let (mut app, _dir, _encodes, _polls) = setup(case);
        let b = draw(&mut app, case.width);
        compare(&records, &case.name(), &b);
        compare(
            &records,
            &case.name(),
            &small(&app, b.area, |_, _, _| false),
        );
        count += 1;
    }
    assert!(count >= 46);
}

fn encode(app: &mut App, encodes: &Receiver<MdEncodeRequest>) -> Vec<Value> {
    let mut requests = Vec::new();
    for (i, req) in encodes.try_iter().enumerate() {
        assert_eq!(
            req.image.to_rgba8().into_raw(),
            [10, 20, 30, 255].repeat(48)
        );
        requests.push(json!({ "key": req.path.to_string_lossy(), "cols": req.cols, "rows": req.rows, "crop": req.crop }));
        let image = InlineImage::Kitty(konoma_widgets::kitty::KittyImage::build(
            &req.image.to_rgba8(),
            req.cols,
            req.rows,
            42 + i as u32,
            false,
        ));
        app.md_images
            .apply_encode(konoma_widgets::inline_images::MdEncodeResult {
                path: req.path,
                key: req.key,
                image: Some(image),
                born: req.born,
            });
    }
    requests
}

fn targets(buf: &Buffer, requests: &mut [Value]) {
    for (i, req) in requests.iter_mut().enumerate() {
        let prefix = format!("\x1b_Gq=2,i={},", 42 + i);
        let (at, _) = buf
            .content
            .iter()
            .enumerate()
            .find(|(_, c)| c.symbol().starts_with(&prefix))
            .expect("kitty transfer at target");
        let x = buf.area.x + at as u16 % buf.area.width;
        let y = buf.area.y + at as u16 / buf.area.width;
        req["target"] = json!([x, y, req["cols"], req["rows"]]);
    }
}

#[test]
fn media_placeholders_use_pane_area_and_keep_failure_reason() {
    let records = fixture();
    let mut compared = 0;
    for case in cases()
        .into_iter()
        .filter(|c| matches!(c.sides, Sides::Placeholders | Sides::Missing))
    {
        let (mut app, _dir, _encodes, _polls) = setup(case);
        compare(&records, &case.name(), &draw(&mut app, case.width));
        compared += 1;
    }
    assert!(compared >= 24);
}

#[test]
fn media_picture_requests_match_keys_rectangles_and_bands() {
    let records = fixture();
    let mut compared = 0;
    for case in cases()
        .into_iter()
        .filter(|c| matches!(c.sides, Sides::Pictures) && c.picker)
    {
        let (mut app, _dir, encodes, polls) = setup(case);
        let pending = draw(&mut app, case.width);
        compare(&records, &case.name(), &pending);
        let mut requests = encode(&mut app, &encodes);
        let ready = draw(&mut app, case.width);
        targets(&ready, &mut requests);
        assert_eq!(json!(requests), records["pictures"][case.name()]);
        compare(&records, &format!("{}-ready", case.name()), &ready);
        let mut paint_requests = Vec::new();
        let sdk = small(&app, ready.area, |key, r, buf| {
            assert_eq!(r.kind, PicturePaintKind::Normal);
            assert_eq!((r.row_offset, r.visible_rows), (0, r.rows));
            assert_eq!((r.target.width, r.target.height), (r.cols, r.rows));
            paint_requests.push(json!({ "key": key, "target": [r.target.x, r.target.y, r.target.width, r.target.height], "cols": r.cols, "rows": r.rows, "crop": null }));
            let id = if key.starts_with("media-diff://o/") {
                42
            } else {
                43
            };
            konoma_widgets::kitty::KittyImage::build(
                &pixels().to_rgba8(),
                r.cols,
                r.rows,
                id,
                false,
            )
            .render(r.target, buf);
            true
        });
        assert_eq!(json!(paint_requests), records["pictures"][case.name()]);
        compare(&records, &format!("{}-ready", case.name()), &sdk);
        let transfer = |b: &Buffer| {
            b.content
                .iter()
                .find_map(|c| {
                    c.symbol()
                        .split_once("\x1b[s")
                        .filter(|(s, _)| s.starts_with("\x1b_Gq=2,i=42,"))
                        .map(|(s, _)| s.to_owned())
                })
                .unwrap()
        };
        assert_eq!(
            transfer(&ready),
            transfer(&sdk),
            "same key and conditions produce the same kitty transfer"
        );
        assert_eq!(
            transfer(&ready),
            konoma_widgets::kitty::build_transmit(&pixels().to_rgba8(), 42, false)
        );
        assert_eq!(polls.try_iter().count(), 1);
        assert!(encodes.try_recv().is_err());
        compared += 1;
    }
    assert!(compared >= 12);
}

#[test]
fn media_missing_cache_key_rekicks_before_drawing() {
    let case = matrix_case();
    let (mut app, _dir, encodes, polls) = setup(case);
    draw(&mut app, case.width);
    let first = polls.try_recv().unwrap();
    let keys = app.md_image_cache_media_diff_keys_for_test();
    assert_eq!(keys.len(), 2);
    encode(&mut app, &encodes);
    app.evict_md_image_cache_key_for_test(&keys[0]);
    assert!(!app.md_image_cache_contains(&keys[0]));
    draw(&mut app, case.width);
    assert_eq!(
        polls
            .try_recv()
            .expect("same-frame re-poll after invalidation"),
        first
    );
    assert!(keys.iter().all(|k| app.md_image_cache_contains(k)));
    assert_eq!(
        encodes.try_recv().expect("evicted side encoded again").path,
        keys[0]
    );
    assert!(
        encodes.try_recv().is_err(),
        "the other side keeps its encoded slot"
    );
    assert!(polls.try_recv().is_err());
}

#[test]
fn media_geometry_uses_intrinsic_dimensions() {
    let case = matrix_case();
    let (mut app, _dir, encodes, _polls) = setup(case);
    let b = draw(&mut app, case.width);
    let mut requests = Vec::new();
    small(&app, b.area, |key, r, _| {
        requests.push((key.to_owned(), r));
        false
    });
    let encodes: Vec<_> = encodes.try_iter().collect();
    assert_eq!(encodes.len(), 2);
    for (encode, (key, r)) in encodes.iter().zip(&requests) {
        assert_eq!(encode.path.to_string_lossy(), *key);
        assert_eq!((encode.cols, encode.rows), (r.cols, r.rows));
        assert_eq!((encode.image.width(), encode.image.height()), (8, 6));
    }
    assert_eq!((requests[0].1.cols, requests[0].1.rows), (8, 4));
    assert_eq!((requests[1].1.cols, requests[1].1.rows), (4, 2));
}

fn matrix_case() -> Case {
    Case {
        sides: Sides::Pictures,
        layout: MediaDiffLayout::Auto,
        lang: Lang::En,
        width: 44,
        picker: true,
    }
}

fn matrix_surface(app: &mut App, surface: &str) {
    if surface == "Media" {
        return;
    }
    let path = app.tab.root.join("doc.md");
    app.tab.preview_path = Some(path.clone());
    app.tab.preview_kind = Some(PreviewKind::GitDiff(path.clone()));
    if surface == "Source" {
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
    } else {
        let old = "old\n";
        let new = "# Heading\n\n  alpha beta gamma delta epsilon zeta eta theta iota kappa lambda\n\nlast\n";
        let old_doc = crate::preview::markdown::model::Doc::parse(old);
        let new_doc = crate::preview::markdown::model::Doc::parse(new);
        let ops = crate::preview::markdown::diff::block_ops(&old_doc, &new_doc, old, new);
        let gen = *app.md_diff.generation_for_test();
        *app.md_diff.landed_for_test() = Some((
            path,
            MdDiffKind::Rendered,
            gen,
            MdDiffOutcome::Ready {
                old_pre: old.into(),
                new_pre: new.into(),
                ops,
                any_change: true,
                front_matter_only: false,
            },
        ));
        app.md_cache = None;
    }
}

#[test]
fn x2_twelve_case_matrix_exercises_every_surface_layout_and_gutter() {
    let records = fixture();
    let mut exercised = std::collections::BTreeSet::new();
    for surface in ["Source", "Rendered", "Media"] {
        let mut previous = None;
        for (layout, label) in [
            (DiffLayout::Unified, "unified"),
            (DiffLayout::Split, "split"),
        ] {
            for gutter in [false, true] {
                let case = matrix_case();
                let (mut app, _dir, encodes, _polls) = setup(case);
                matrix_surface(&mut app, surface);
                app.diff_layout = layout;
                app.cfg.ui.git_gutter = gutter;
                let b = draw(&mut app, case.width);
                let name = format!("matrix-{surface}-{label}-{gutter}");
                compare(&records, &name, &b);
                if surface == "Media" {
                    let mut requests = encode(&mut app, &encodes);
                    let ready = draw(&mut app, case.width);
                    targets(&ready, &mut requests);
                    assert_eq!(json!(requests), records["pictures"][&name]);
                    compare(&records, &format!("{name}-ready"), &ready);
                    if let Some((p, r)) = &previous {
                        assert_eq!(
                            (&b, &requests),
                            (p, r),
                            "line layout and gutter leave media unchanged"
                        );
                    }
                    previous = Some((b, requests));
                }
                assert!(exercised.insert((surface, label, gutter)));
            }
        }
    }
    let expected = [
        ("Source", "unified", false),
        ("Source", "unified", true),
        ("Source", "split", false),
        ("Source", "split", true),
        ("Rendered", "unified", false),
        ("Rendered", "unified", true),
        ("Rendered", "split", false),
        ("Rendered", "split", true),
        ("Media", "unified", false),
        ("Media", "unified", true),
        ("Media", "split", false),
        ("Media", "split", true),
    ]
    .into_iter()
    .collect();
    assert_eq!(exercised, expected);
    assert_eq!(exercised.len(), 12);
}
