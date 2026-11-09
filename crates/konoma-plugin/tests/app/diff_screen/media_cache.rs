use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc};

use konoma_host_core::project::DiffTarget;
use konoma_plugin::app::{DocPics, InProcessSvg, Laid};
use konoma_plugin::host::project::md_diff::DiffBaseline;
use konoma_plugin::host::project::media_diff::*;
use konoma_widgets::inline_images::{InlineImageCache, MdEncodeRequest};
use konoma_widgets::markdown::ImagePlacement;
use ratatui::{buffer::Buffer, layout::Rect};
use serde_json::Value;

#[path = "media_loop.rs"]
mod looping;

fn fixture() -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../snapshots/media_diff_cache.json");
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
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

fn computed(f: &Value) -> MediaDiffComputed {
    let dir = konoma_test_support::unique_tmp("media_cache");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("side.svg");
    std::fs::write(&path, f["sides"][1]["source"].as_str().unwrap()).unwrap();
    MediaDiffWorker::compute_media_diff(&MediaDiffRequest {
        gen: 0,
        path,
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
        classify: Arc::new(|_, _| DiffTarget::Svg),
        rasterize_svg: fixed_svg,
        cancel: Arc::default(),
    })
}

fn pictures() -> (DocPics, mpsc::Receiver<MdEncodeRequest>) {
    pictures_with(InlineImageCache::default())
}

fn pictures_with(mut cache: InlineImageCache) -> (DocPics, mpsc::Receiver<MdEncodeRequest>) {
    let picker = ratatui_image::picker::Picker::halfblocks();
    let (tx, rx) = mpsc::channel();
    cache.attach_picker(&picker);
    cache.enc_tx = Some(tx);
    let (_, enc_rx) = mpsc::channel();
    let pics = DocPics::new(
        &picker,
        "test",
        Path::new("/fixture/doc.md"),
        128,
        Arc::new(InProcessSvg),
    )
    .with_cache(cache, enc_rx);
    (pics, rx)
}

#[test]
fn media_cache_prunes_only_obsolete_media_keys() {
    use konoma_widgets::inline_images::MdImgEntry;
    let f = fixture();
    let keys = [
        "mermaid-fence://keep",
        "math://keep",
        "/fixture/keep.png",
        "media-diff://obsolete",
    ];
    let mut cache = InlineImageCache::default();
    let mut tickets = Vec::new();
    for key in keys {
        let entry = MdImgEntry {
            failed: true,
            ..Default::default()
        };
        tickets.push(entry.wish.ticket());
        cache.cache.insert(key.into(), entry);
    }
    let (mut pics, _) = pictures_with(cache);
    pics.materialize_media_diff(computed(&f));
    assert!(
        tickets[3].is_stale(),
        "obsolete media key is pruned on adoption"
    );
    assert!(
        tickets[..3].iter().all(|t| !t.is_stale()),
        "adoption leaves mermaid, math and file keys alone"
    );
    pics.prune_media_diff_picture_cache();
    assert!(
        tickets[..3].iter().all(|t| !t.is_stale()),
        "departure leaves mermaid, math and file keys alone"
    );
    for side in f["sides"].as_array().unwrap() {
        draw(&mut pics, side["key"].as_str().unwrap());
    }
    assert!(
        !pics.loading(),
        "departed media must not be decoded from a synthetic file path"
    );

    // Observe the real q transition while Pictures still owns its cache: dropping the whole
    // Pictures at loop exit would also cancel the ticket and conceal a missing departure prune.
    let dir = konoma_test_support::unique_tmp("media_departure");
    let mut prepared = looping::prepared(&dir);
    let mut pictures = konoma_plugin::app::Pictures::new(
        ratatui_image::picker::Picker::halfblocks(),
        "fixture",
        &dir.join("side.svg"),
        128,
    )
    .with_out(Box::new(std::io::sink()));
    let d = pictures.doc_pics();
    let mut cache = InlineImageCache::default();
    let key = PathBuf::from(f["sides"][0]["key"].as_str().unwrap());
    let entry = MdImgEntry::default();
    let ticket = entry.wish.ticket();
    cache.cache.insert(key, entry);
    let (_, rx) = mpsc::channel();
    *d = DocPics::new(
        &ratatui_image::picker::Picker::halfblocks(),
        "fixture",
        &dir.join("side.svg"),
        128,
        Arc::new(InProcessSvg),
    )
    .with_cache(cache, rx);
    d.materialize_media_diff(computed(&f));
    struct Quit {
        ticket: Arc<konoma_svg::image::DecodeTicket>,
        sent: bool,
    }
    impl konoma_plugin::app::EventSource for Quit {
        fn next(&mut self, _: Option<std::time::Duration>) -> konoma_plugin::app::Next {
            use konoma_plugin::app::{AppEvent, Next};
            if !self.sent {
                assert!(!self.ticket.is_stale(), "media is resident before q");
                self.sent = true;
                Next::Event(AppEvent::Key(konoma_plugin::Key::char('q')))
            } else {
                assert!(
                    self.ticket.is_stale(),
                    "q prunes media before the next project frame"
                );
                Next::End
            }
        }
    }
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(40, 10)).unwrap();
    konoma_plugin::app::project::run_project_with(
        &mut terminal,
        &mut prepared,
        &mut Quit {
            ticket,
            sent: false,
        },
        &Default::default(),
        Some(pictures),
    )
    .unwrap();
}

fn draw(pics: &mut DocPics, key: &str) {
    let laid = Laid {
        lines: vec!["".into(); 4],
        row_prefix: vec![],
        max_line_cols: 4,
        images: vec![ImagePlacement {
            url: key.into(),
            alt: String::new(),
            line: 0,
            col: 0,
            cols: 4,
            rows: 4,
            fence_ord: None,
        }],
    };
    let area = Rect::new(0, 0, 8, 4);
    pics.draw(&mut Buffer::empty(area), area, Some((&laid, 1)));
}

#[test]
fn media_results_land_under_exact_keys_with_fixed_rgba() {
    let f = fixture();
    let (mut pics, encodes) = pictures();
    let outcome = pics.materialize_media_diff(computed(&f));
    sync_document_resources(&mut pics);
    let MediaDiffOutcome::Ready {
        old: MediaDiffSide::Picture(old),
        new: MediaDiffSide::Picture(new),
        ..
    } = outcome
    else {
        panic!("two pictures expected")
    };
    for (p, expected) in [old, new].iter().zip(f["sides"].as_array().unwrap()) {
        assert_eq!(
            p.cache_key,
            PathBuf::from(expected["key"].as_str().unwrap()),
            "exact side/hash/page/raster key"
        );
        assert_eq!(
            p.natural_px,
            (
                expected["natural"][0].as_u64().unwrap() as u32,
                expected["natural"][1].as_u64().unwrap() as u32
            ),
            "intrinsic size survives the fixed 2x1 raster"
        );
        assert_eq!(p.page_count, None);
        let (_, paints) = konoma_widgets::picture_overlay::record_inline_paints_for_test(|| {
            draw(&mut pics, expected["key"].as_str().unwrap())
        });
        assert_eq!(paints.len(), 1);
        assert_eq!(paints[0].0, expected["key"].as_str().unwrap());
        let r = paints[0].1;
        assert_eq!(
            serde_json::json!({"target": [r.target.x, r.target.y, r.target.width, r.target.height], "cols": r.cols, "rows": r.rows, "offset": r.row_offset, "visible": r.visible_rows, "crop": [0, 0, 2, 1]}),
            f["placement"]
        );
        let encode = encodes
            .try_recv()
            .expect("pre-placed pixels must encode immediately without decoding the key as a file");
        assert_eq!(encode.path, p.cache_key);
        assert_eq!(
            encode.image.to_rgba8().into_raw(),
            expected["rgba"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_u64().unwrap() as u8)
                .collect::<Vec<_>>()
        );
        assert_eq!(
            u64::from(encode.cols),
            f["placement"]["cols"].as_u64().unwrap()
        );
        assert_eq!(
            u64::from(encode.rows),
            f["placement"]["visible"].as_u64().unwrap()
        );
        let crop = encode.crop.unwrap();
        assert_eq!(
            serde_json::json!([crop.0, crop.1, crop.2, crop.3]),
            f["placement"]["crop"]
        );
    }
}

fn sync_document_resources(pics: &mut DocPics) {
    use konoma_plugin::{
        input::{Origin, Resource, Resources},
        view::{DocOptions, DocumentView},
    };
    let mut resources = Resources::new();
    resources.insert(
        "office-img://unused",
        Resource::Bytes(Arc::from(&b"unused"[..])),
    );
    let doc = DocumentView {
        markdown: "ordinary document".into(),
        origin: Origin::File("/fixture/doc.md".into()),
        resources,
        options: DocOptions::default(),
        title: None,
        truncated: false,
    };
    let details_open = Default::default();
    pics.render(
        &doc,
        &doc.markdown,
        20,
        10,
        konoma_host_core::lang::Lang::En,
        &konoma_plugin::document_render::RenderSettings {
            width: 20,
            code: konoma_widgets::markdown::CodeStyle::default(),
            code_theme: "base16-ocean.dark",
            icons: false,
            mermaid_caption: "",
            details_open: &details_open,
        },
    );
}

#[test]
fn media_svg_rebuild_keeps_file_trust_and_base() {
    use konoma_plugin::canvas_pictures::SvgDraw;
    use konoma_widgets::inline_images::SvgTrust;
    struct Child(mpsc::Sender<PathBuf>);
    impl SvgDraw for Child {
        fn untrusted(
            &self,
            data: &[u8],
            base: &Path,
            roots: Option<&[PathBuf]>,
            max: u32,
            cancelled: &dyn Fn() -> bool,
        ) -> Result<image::DynamicImage, konoma_svg::svg::RasterError> {
            assert!(roots.is_none());
            assert_eq!(max, 128);
            self.0.send(base.into()).unwrap();
            fixed_svg(data, base, max, cancelled)
        }
        fn embedded(
            &self,
            _: &[u8],
            _: u32,
            _: &dyn Fn() -> bool,
        ) -> Result<image::DynamicImage, konoma_svg::svg::RasterError> {
            panic!("a media SVG must use the file child, never embedded drawing")
        }
    }
    let f = fixture();
    let (tx, child_calls) = mpsc::channel();
    let (results, replies) = mpsc::channel();
    let mut cache = InlineImageCache::default();
    let (enc_tx, encodes) = mpsc::channel();
    cache.enc_tx = Some(enc_tx);
    let mut inflight = Default::default();
    let mut failed = Default::default();
    fn requests<'a>(
        cache: &'a mut InlineImageCache,
        inflight: &'a mut std::collections::HashSet<String>,
        failed: &'a mut std::collections::HashSet<String>,
        results: &mpsc::Sender<konoma_widgets::inline_images::DecodedImage>,
    ) -> konoma_plugin::document_pictures::DocPictures<'a> {
        konoma_plugin::document_pictures::DocPictures {
            cache,
            remote_inflight: inflight,
            remote_failed: failed,
            results: Some(results.clone()),
            fetches: None,
            remote_images: false,
            fetcher: konoma_plugin::document_pictures::fetch_remote_image,
        }
    }
    let decoded = computed(&f);
    let MediaDiffComputed::Ready {
        old: MediaDiffSideDecoded::Picture(ref old),
        ..
    } = decoded
    else {
        panic!("SVG expected")
    };
    let original_base = old.svg_base.clone().unwrap();
    materialize_media_diff(decoded, |p| {
        requests(&mut cache, &mut inflight, &mut failed, &results).place_media_diff_picture(p);
    });
    let key = PathBuf::from(f["sides"][0]["key"].as_str().unwrap());
    let entry = &cache.cache[&key];
    assert_eq!(
        entry.svg_trust,
        SvgTrust::File(original_base.clone()),
        "adoption keeps the worker's original file base"
    );
    assert_eq!(entry.layout_px, Some((40, 20)));
    cache.evict_to(0);
    assert!(cache.cache[&key].decoded.is_none());
    requests(&mut cache, &mut inflight, &mut failed, &results).rebuild_media_svg(
        &key,
        128,
        Arc::new(Child(tx)),
        (0, Arc::default()),
    );
    assert!(
        encodes.try_recv().is_err(),
        "evicted pixels wait for the child"
    );
    let result = replies
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("child publishes the rebuilt pixels");
    requests(&mut cache, &mut inflight, &mut failed, &results).apply_result(result);
    assert_eq!(
        child_calls.try_recv().ok(),
        Some(original_base.clone()),
        "same file base reaches the child on rebuild"
    );
    let entry = &cache.cache[&key];
    assert_eq!(entry.svg_trust, SvgTrust::File(original_base));
    assert_eq!(
        entry.layout_px,
        Some((40, 20)),
        "rebuild keeps natural size"
    );
    cache.ensure_encode(&key, 4, 4, 1, 3, None);
    assert_eq!(
        encodes.try_recv().unwrap().image.to_rgba8().into_raw(),
        f["sides"][0]["rgba"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_u64().unwrap() as u8)
            .collect::<Vec<_>>(),
        "the supervised child's pixels reach the encoder"
    );
    assert!(child_calls.try_recv().is_err(), "one rebuild only");

    #[cfg(feature = "test-hooks")]
    {
        // Also exercise the standalone adapter's choice of drawer, beyond the borrowed cache API.
        let (tx, calls) = mpsc::channel();
        let (mut pics, encodes) = pictures();
        pics = pics.with_media_svg(Arc::new(Child(tx)));
        pics.materialize_media_diff(computed(&f));
        pics.cache_for_test().evict_to(0);
        draw(&mut pics, key.to_str().unwrap());
        pics.await_decode_for_test();
        assert!(
            calls.try_recv().is_ok(),
            "DocPics rebuild calls the supervised child"
        );
        assert_eq!(pics.cache_for_test().cache[&key].layout_px, Some((40, 20)));
        draw(&mut pics, key.to_str().unwrap());
        assert_eq!(
            encodes.try_recv().unwrap().image.to_rgba8().into_raw(),
            f["sides"][0]["rgba"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_u64().unwrap() as u8)
                .collect::<Vec<_>>()
        );
    }
}
