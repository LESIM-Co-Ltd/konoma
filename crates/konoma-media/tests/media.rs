use std::path::Path;

use konoma_media::{
    ImagePlugin, PdfPlugin, SvgPlugin, VideoPlugin, IMAGE_ID, PDF_ID, SVG_ID, SVG_MAX_PX, VIDEO_ID,
};
use konoma_plugin::view::{ImageData, ImageView, PageLayout, View, ViewReq};
use konoma_plugin::{Cancel, FailureKind, Input, OpenCx, Plugin, Session, Settings, Value, ViewCx};
use konoma_test_support::{sample_path_or_skip, unique_tmp};

fn open(plugin: &dyn Plugin, path: &Path) -> Result<Box<dyn Session>, konoma_plugin::Failure> {
    konoma_plugin::testing::open_file(plugin, path)
}

fn image_of(view: View) -> ImageView {
    match view {
        View::Image(v) => v,
        other => panic!("an image view was expected, got {other:?}"),
    }
}

#[test]
fn the_manifests_name_what_each_plugin_shows() {
    use konoma_plugin::Target;
    let has_files = |m: &konoma_plugin::Manifest, glob: &str| {
        m.binds
            .iter()
            .flat_map(|b| &b.targets)
            .any(|t| matches!(t, Target::Glob(g) if g == glob))
    };
    let has_mime = |m: &konoma_plugin::Manifest, mime: &str| {
        m.binds
            .iter()
            .flat_map(|b| &b.targets)
            .any(|t| matches!(t, Target::Mime(x) if x == mime))
    };
    let (i, s, p, v) = (
        ImagePlugin.manifest(),
        SvgPlugin.manifest(),
        PdfPlugin.manifest(),
        VideoPlugin.manifest(),
    );
    assert_eq!(
        [&i.id, &s.id, &p.id, &v.id],
        [IMAGE_ID, SVG_ID, PDF_ID, VIDEO_ID]
    );
    assert!(has_mime(&i, "image/*"));
    assert!(has_files(&s, "*.svg"));
    assert!(has_files(&p, "*.pdf"));
    assert!(has_mime(&v, "video/*"));
    // The SVG plugin declares the setting the host fills from the old `[ui] svg_max_px`.
    assert!(s.settings.iter().any(|d| d.name == "max_px"));
    // Every one of them can be edited and opened elsewhere.
    for m in [&i, &s, &p, &v] {
        assert!(m.verbs.len() >= 2, "{}", m.id);
    }
}

#[test]
fn the_plugins_open_files_only() {
    let embed = || Input::Embed {
        kind: "x".into(),
        source: String::new(),
        display: false,
    };
    for plugin in [
        &ImagePlugin as &dyn Plugin,
        &SvgPlugin,
        &PdfPlugin,
        &VideoPlugin,
    ] {
        // (`Box<dyn Session>` is not `Debug`, so `expect_err` is not available here.)
        let Err(err) = plugin.open(embed(), &OpenCx::bare()) else {
            panic!("an embed is not a file");
        };
        assert_eq!(err.kind, FailureKind::Unsupported);
    }
}

#[test]
fn an_image_is_the_file_itself_for_the_host_to_decode() {
    let dir = unique_tmp("konoma_media_image");
    std::fs::create_dir_all(&dir).unwrap();
    let png = dir.join("a.png");
    // The plugin does not read the file: even a name that is not an image gives the file view.
    std::fs::write(&png, b"not a png").unwrap();
    let mut s = open(&ImagePlugin, &png).unwrap();
    assert!(s.layout().pages.is_none());
    let v = image_of(s.view(&ViewReq::default(), &ViewCx::bare()).unwrap());
    assert!(matches!(&v.data, ImageData::File(p) if *p == png));
    assert!(!v.sharp_zoom);
    assert_eq!(v.raster_max_px, None);
}

const SVG: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><rect width="10" height="10"/></svg>"#;

#[test]
fn an_svg_is_untrusted_sharp_zoomable_and_carries_the_raster_size() {
    let dir = unique_tmp("konoma_media_svg");
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("a.svg");
    std::fs::write(&p, SVG).unwrap();
    let mut s = open(&SvgPlugin, &p).unwrap();
    let v = image_of(s.view(&ViewReq::default(), &ViewCx::bare()).unwrap());
    match &v.data {
        ImageData::Svg {
            data,
            base_dir,
            trusted,
        } => {
            assert_eq!(&data[..], SVG.as_bytes());
            assert_eq!(base_dir.as_deref(), Some(dir.as_path()));
            assert!(!trusted, "a file the user opened is never trusted");
        }
        other => panic!("{other:?}"),
    }
    assert!(v.sharp_zoom);
    assert_eq!(v.raster_max_px, Some(SVG_MAX_PX as u32));
    // `max_px` is the setting of the plugin.
    let settings = Settings::new().with("max_px", Value::Int(1234));
    let v = image_of(
        s.view(&ViewReq::default(), &ViewCx::new(&settings, "en"))
            .unwrap(),
    );
    assert_eq!(v.raster_max_px, Some(1234));
    // Nonsense is held to a sane range instead of reaching the rasterizer.
    let settings = Settings::new().with("max_px", Value::Int(-5));
    let v = image_of(
        s.view(&ViewReq::default(), &ViewCx::new(&settings, "en"))
            .unwrap(),
    );
    assert_eq!(v.raster_max_px, Some(1));
    let settings = Settings::new().with("max_px", Value::Int(10_000_000));
    let v = image_of(
        s.view(&ViewReq::default(), &ViewCx::new(&settings, "en"))
            .unwrap(),
    );
    assert_eq!(v.raster_max_px, Some(16384));
}

#[test]
fn an_svg_that_cannot_be_read_says_why() {
    let dir = unique_tmp("konoma_media_svg_limits");
    std::fs::create_dir_all(&dir).unwrap();
    let big = dir.join("big.svg");
    std::fs::File::create(&big)
        .unwrap()
        .set_len(33 << 20)
        .unwrap();
    let mut s = open(&SvgPlugin, &big).unwrap();
    let f = s
        .view(&ViewReq::default(), &ViewCx::bare())
        .expect_err("a 33 MiB file is refused");
    assert_eq!(f.kind, FailureKind::TooLarge);
    assert_eq!(
        f.message.get("en"),
        "[svg] not drawn: the file is too large"
    );
    assert_eq!(
        f.message.get("ja"),
        "[svg] 描画しません: ファイルが大きすぎます"
    );
    // A file that is not there is the "damaged" reason, not a panic.
    let mut s = open(&SvgPlugin, &dir.join("missing.svg")).unwrap();
    let f = s
        .view(&ViewReq::default(), &ViewCx::bare())
        .expect_err("nothing to read");
    assert_eq!(f.kind, FailureKind::Corrupt);
}

#[test]
fn a_pdf_reports_its_pages_and_draws_the_one_asked_for() {
    let Some(pdf) = sample_path_or_skip("sample.pdf") else {
        return;
    };
    let mut s = open(&PdfPlugin, &pdf).unwrap();
    let layout = s.layout();
    assert_eq!(
        layout.pages,
        Some(PageLayout::Separate {
            count: Some(3),
            titles: Vec::new()
        })
    );
    let dims = |s: &mut Box<dyn Session>, page: Option<usize>| {
        let mut req = ViewReq::new(80, 24);
        req.page = page;
        match image_of(s.view(&req, &ViewCx::bare()).unwrap()).data {
            ImageData::Rgba {
                width,
                height,
                pixels,
            } => {
                assert_eq!(pixels.len(), (width * height * 4) as usize);
                (width, height)
            }
            other => panic!("{other:?}"),
        }
    };
    let first = dims(&mut s, None);
    assert_eq!(dims(&mut s, Some(0)), first, "no page means the first");
    assert!(first.0 > 0 && first.1 > 0);
    // A page past the end is the last page, not a failure.
    let last = dims(&mut s, Some(2));
    assert_eq!(dims(&mut s, Some(99)), last);
}

#[test]
fn a_pdf_that_cannot_be_drawn_fails_with_the_unavailable_text_and_still_opens() {
    let dir = unique_tmp("konoma_media_pdf_bad");
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("bad.pdf");
    std::fs::write(&p, b"%PDF-1.4\nnot really").unwrap();
    // Opening never fails on a bad file: the page count is just unknown (navigation off).
    let mut s = open(&PdfPlugin, &p).unwrap();
    assert_eq!(
        s.layout().pages,
        Some(PageLayout::Separate {
            count: None,
            titles: Vec::new()
        })
    );
    // `external` off: nothing may be started to rescue it.
    let f = s
        .view(&ViewReq::default(), &ViewCx::bare().with_external(false))
        .expect_err("a broken PDF has no picture");
    assert_eq!(f.kind, FailureKind::Unsupported);
    assert!(f.message.get("en").starts_with("[pdf] cannot render"));
    assert!(f.message.get("ja").starts_with("[PDF] 表示不可"));
}

#[test]
fn a_cancelled_pdf_or_video_draws_nothing() {
    let Some(pdf) = sample_path_or_skip("sample.pdf") else {
        return;
    };
    let cx = ViewCx::bare().with_cancel(Cancel::from_fn(|| true));
    let mut s = open(&PdfPlugin, &pdf).unwrap();
    let f = s.view(&ViewReq::default(), &cx).expect_err("cancelled");
    assert_eq!(f.kind, FailureKind::Cancelled);
    let mut s = open(&VideoPlugin, &pdf).unwrap();
    let f = s.view(&ViewReq::default(), &cx).expect_err("cancelled");
    assert_eq!(f.kind, FailureKind::Cancelled);
}

#[test]
fn a_video_with_no_thumbnail_fails_with_the_install_hint() {
    let dir = unique_tmp("konoma_media_video_bad");
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("v.mp4");
    std::fs::write(&p, b"not a video").unwrap();
    let mut s = open(&VideoPlugin, &p).unwrap();
    assert!(s.layout().pages.is_none());
    // `external` is off by default: no ffmpeg is started for a file the built-in decoder declines.
    let f = s
        .view(&ViewReq::default(), &ViewCx::bare())
        .expect_err("nothing to extract");
    assert_eq!(f.kind, FailureKind::External);
    assert!(f.message.get("en").starts_with("[video] no thumbnail"));
    assert!(f.message.get("ja").starts_with("[動画] サムネイル不可"));
}
