//! Pin the shared pixel fixture through konoma's real computation and cache adoption.
use super::*;
use serde_json::Value;

fn fixture() -> Value {
    serde_json::from_str(
        &std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("snapshots/media_diff_cache.json"),
        )
        .unwrap(),
    )
    .unwrap()
}

fn fixed_svg(
    data: &[u8],
    _: &Path,
    _: u32,
    _: &dyn Fn() -> bool,
) -> Result<image::DynamicImage, konoma_svg::svg::RasterError> {
    let f = fixture();
    let side = f["sides"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["source"].as_str().unwrap().as_bytes() == data)
        .unwrap();
    let rgba = side["rgba"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_u64().unwrap() as u8)
        .collect();
    Ok(image::DynamicImage::ImageRgba8(
        image::RgbaImage::from_raw(2, 1, rgba).unwrap(),
    ))
}

#[test]
fn host_media_cache_matches_fixed_rgba_reference() {
    let f = fixture();
    let dir = crate::test_support::unique_tmp("media_cache_bridge");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("side.svg");
    std::fs::write(&path, f["sides"][1]["source"].as_str().unwrap()).unwrap();
    let request = MediaDiffRequest {
        gen: 0,
        path: path.clone(),
        root: dir.to_path_buf(),
        baseline: DiffBaseline::FollowSnapshot(
            f["sides"][0]["source"]
                .as_str()
                .unwrap()
                .as_bytes()
                .to_vec(),
        ),
        page: 2,
        raster_px: (64, 128),
        classify: media_classifier(Config::default().preview.rules, Vec::new(), true),
        rasterize_svg: fixed_svg,
        cancel: Arc::default(),
    };
    let computed = App::compute_media_diff(&request);
    let mut app = App::new(dir.to_path_buf(), Config::default()).unwrap();
    let (enc_tx, enc_rx) = std::sync::mpsc::channel();
    app.attach_md_encoder(enc_tx);
    assert!(app.apply_media_diff(MediaDiffResult {
        gen: 0,
        path: path.clone(),
        page: 2,
        raster_px: (64, 128),
        computed
    }));
    let MediaDiffOutcome::Ready {
        old: MediaDiffSide::Picture(old),
        new: MediaDiffSide::Picture(new),
        ..
    } = app.media_diff.outcome_for(&path).unwrap().clone()
    else {
        panic!("two pictures expected")
    };
    for (p, expected) in [old, new].into_iter().zip(f["sides"].as_array().unwrap()) {
        assert_eq!(
            p.cache_key,
            PathBuf::from(expected["key"].as_str().unwrap())
        );
        assert_eq!(
            p.natural_px,
            (
                expected["natural"][0].as_u64().unwrap() as u32,
                expected["natural"][1].as_u64().unwrap() as u32
            )
        );
        assert_eq!(p.page_count, None);
        let entry = &app.md_images.cache[&p.cache_key];
        assert_eq!(entry.svg_trust, SvgTrust::File(path.clone()));
        assert_eq!(
            entry.decoded.as_ref().unwrap().to_rgba8().into_raw(),
            expected["rgba"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_u64().unwrap() as u8)
                .collect::<Vec<_>>()
        );
        use konoma_widgets::picture_overlay::{
            draw_inline_pictures, InlineOverlayValues, InlinePicturePlacement,
        };
        let url = p.cache_key.to_str().unwrap();
        let inner = ratatui::layout::Rect::new(0, 0, 8, 4);
        app.begin_md_image_frame();
        draw_inline_pictures(
            &mut ratatui::buffer::Buffer::empty(inner),
            [InlinePicturePlacement {
                url,
                visual_row: 0,
                col: 0,
                cols: 4,
                rows: 4,
                fence_ord: None,
            }],
            InlineOverlayValues {
                inner,
                scroll: 1,
                gutter: 0,
                focused_ordinal: None,
                zoom: 1.0,
                image_align: Default::default(),
                focus_title: "",
            },
            |key, r, _| {
                assert_eq!(key, expected["key"].as_str().unwrap());
                assert_eq!(
                    serde_json::json!({"target": [r.target.x, r.target.y, r.target.width, r.target.height], "cols": r.cols, "rows": r.rows, "offset": r.row_offset, "visible": r.visible_rows, "crop": [0, 0, 2, 1]}),
                    f["placement"]
                );
                app.ensure_md_image(key, r.cols, r.rows, r.row_offset, r.visible_rows);
                false
            },
        );
        let encoded = enc_rx.try_recv().expect("konoma encodes pre-placed pixels");
        assert_eq!(encoded.path, p.cache_key);
        assert_eq!(
            u64::from(encoded.cols),
            f["placement"]["cols"].as_u64().unwrap()
        );
        assert_eq!(
            u64::from(encoded.rows),
            f["placement"]["visible"].as_u64().unwrap()
        );
        let crop = encoded.crop.unwrap();
        assert_eq!(
            serde_json::json!([crop.0, crop.1, crop.2, crop.3]),
            f["placement"]["crop"]
        );
        assert_eq!(
            encoded.image.to_rgba8().into_raw(),
            expected["rgba"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_u64().unwrap() as u8)
                .collect::<Vec<_>>()
        );
    }
}

#[test]
fn shared_gutter_colours_preserve_konoma_gutters() {
    use crate::preview::markdown::DiffMark;
    use ratatui::style::Color;
    for (mark, diff_mark, expected) in [
        (GutterMark::Added, DiffMark::Added, Color::Rgb(87, 171, 90)),
        (
            GutterMark::Modified,
            DiffMark::Modified,
            Color::Rgb(216, 166, 74),
        ),
        (
            GutterMark::Deleted,
            DiffMark::Removed,
            Color::Rgb(199, 84, 80),
        ),
    ] {
        let out = with_git_gutter(vec!["line".into()], 0, &[(1, mark)].into());
        assert_eq!(
            out[0].spans[0].style.fg,
            Some(expected),
            "konoma gutter colour"
        );
        let out = with_diff_gutter(vec!["line".into()], 0, &[(0..1, diff_mark)]);
        assert_eq!(
            out[0].spans[0].style.fg,
            Some(expected),
            "rendered gutter colour"
        );
    }
}
