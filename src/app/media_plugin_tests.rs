//! The picture plugins on the host side (`app/media_plugin.rs`): the work of the plugin and the
//! decoding happen in one go on the media worker, the session survives for the next page, a stale
//! result is dropped, the `external` switch and the other contexts reach the plugin, and a plugin
//! that is switched off shows nothing. The plugins here are stand-ins written for the test where a
//! property has to be observed on its own; the real `image`/`svg`/`pdf` plugins are used where the
//! property is about them.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;

use konoma_plugin::view::{ImageData, ImageView, Layout, PageLayout, View, ViewReq};
use konoma_plugin::{
    t, Failure, FailureKind, Input, Manifest, OpenCx, Plugin, Session, Settings, Value, ViewCx,
};

use super::*;
use crate::test_support::{sample_path_or_skip, unique_tmp};

// ---- stand-in plugin ------------------------------------------------------------------------

/// What the stand-in saw, shared with the test.
#[derive(Default)]
struct Seen {
    opens: AtomicUsize,
    views: AtomicUsize,
    open_external: AtomicBool,
    view_external: AtomicBool,
    open_pixels: AtomicBool,
    view_thread: Mutex<Option<std::thread::ThreadId>>,
    pages: Mutex<Vec<Option<usize>>>,
    settings: Mutex<Option<Settings>>,
    lang: Mutex<String>,
}

#[derive(Clone)]
enum Reply {
    /// A 4x3 picture of raw pixels.
    Picture,
    /// A view that is not a picture.
    Text,
    /// Pixels that do not add up.
    BadPixels,
    /// An SVG that asks for no sharp zoom.
    PlainSvg,
    Fail(FailureKind),
}

struct Stand {
    seen: Arc<Seen>,
    reply: Reply,
    pages: Option<usize>,
}

struct StandSession {
    seen: Arc<Seen>,
    reply: Reply,
    pages: Option<usize>,
}

impl Plugin for Stand {
    fn manifest(&self) -> Manifest {
        Manifest::new("stand", "0")
    }

    fn open(&self, _input: Input, cx: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        self.seen.opens.fetch_add(1, Ordering::SeqCst);
        self.seen.open_external.store(cx.external, Ordering::SeqCst);
        self.seen
            .open_pixels
            .store(cx.term.pixels, Ordering::SeqCst);
        *self.seen.settings.lock().unwrap() = Some(cx.settings.clone());
        *self.seen.lang.lock().unwrap() = cx.lang.to_string();
        Ok(Box::new(StandSession {
            seen: self.seen.clone(),
            reply: self.reply.clone(),
            pages: self.pages,
        }))
    }
}

impl Session for StandSession {
    fn layout(&self) -> Layout {
        match self.pages {
            Some(n) => Layout {
                pages: Some(PageLayout::Separate {
                    count: Some(n),
                    titles: Vec::new(),
                }),
                ..Layout::single()
            },
            None => Layout::single(),
        }
    }

    fn view(&mut self, req: &ViewReq, cx: &ViewCx) -> Result<View, Failure> {
        self.seen.views.fetch_add(1, Ordering::SeqCst);
        self.seen.view_external.store(cx.external, Ordering::SeqCst);
        *self.seen.view_thread.lock().unwrap() = Some(std::thread::current().id());
        self.seen.pages.lock().unwrap().push(req.page);
        match &self.reply {
            Reply::Picture => Ok(View::Image(ImageView::new(ImageData::Rgba {
                width: 4,
                height: 3,
                pixels: vec![200u8; 4 * 3 * 4].into(),
            }))),
            Reply::Text => Ok(View::text(["not a picture"])),
            Reply::BadPixels => Ok(View::Image(ImageView::new(ImageData::Rgba {
                width: 4,
                height: 3,
                pixels: vec![0u8; 5].into(),
            }))),
            Reply::PlainSvg => Ok(View::Image(ImageView {
                data: ImageData::Svg {
                    data: SVG.as_bytes().into(),
                    base_dir: None,
                    trusted: true,
                },
                sharp_zoom: false,
                raster_max_px: Some(64),
            })),
            Reply::Fail(kind) => Err(Failure::new(*kind, t!(en: "nope", ja: "だめ"))),
        }
    }
}

const SVG: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><rect width="10" height="10"/></svg>"#;

fn stand(reply: Reply, pages: Option<usize>) -> (Arc<dyn Plugin>, Arc<Seen>) {
    let seen = Arc::new(Seen::default());
    (
        Arc::new(Stand {
            seen: seen.clone(),
            reply,
            pages,
        }),
        seen,
    )
}

fn job(plugin: Arc<dyn Plugin>, id: &str) -> MediaPluginJob {
    MediaPluginJob {
        plugin,
        id: id.to_string(),
        path: PathBuf::from("/x/file.bin"),
        session: None,
        page: 1,
        req: ViewReq::new(80, 24),
        settings: Settings::new(),
        lang: "en",
        pixels: false,
        external: false,
        default_raster_px: 800,
    }
}

fn landed(payload: MediaPayload) -> Box<PluginLanded> {
    match payload {
        MediaPayload::Plugin(l) => l,
        _ => panic!("a picture plugin job always delivers a landing"),
    }
}

fn image_payload(payload: MediaPayload) -> Option<MediaPayload> {
    landed(payload).into_image_payload()
}

// ---- the job --------------------------------------------------------------------------------

#[test]
fn the_contexts_reach_the_plugin_in_open_and_view() {
    for external in [false, true] {
        let (plugin, seen) = stand(Reply::Picture, None);
        let mut j = job(plugin, "stand");
        j.external = external;
        j.pixels = true;
        j.lang = "ja";
        j.settings = Settings::new().with("k", Value::Int(7));
        let out = j.run(None);
        assert!(matches!(
            image_payload(out),
            Some(MediaPayload::Static(img)) if (img.width(), img.height()) == (4, 3)
        ));
        assert_eq!(seen.opens.load(Ordering::SeqCst), 1);
        assert_eq!(seen.views.load(Ordering::SeqCst), 1);
        assert_eq!(seen.open_external.load(Ordering::SeqCst), external);
        assert_eq!(seen.view_external.load(Ordering::SeqCst), external);
        assert!(seen.open_pixels.load(Ordering::SeqCst));
        assert_eq!(seen.lang.lock().unwrap().as_str(), "ja");
        assert_eq!(
            seen.settings.lock().unwrap().as_ref().unwrap().int("k", 0),
            7
        );
    }
}

#[test]
fn the_page_asked_for_is_counted_from_zero_in_the_request() {
    let (plugin, seen) = stand(Reply::Picture, Some(5));
    let mut j = job(plugin, "stand");
    j.page = 3;
    let _ = j.run(None);
    assert_eq!(*seen.pages.lock().unwrap(), vec![Some(2)]);
}

#[test]
fn the_session_is_handed_back_and_a_later_job_reuses_it_without_opening_again() {
    let (plugin, seen) = stand(Reply::Picture, Some(3));
    let first = landed(job(plugin.clone(), "stand").run(None));
    assert_eq!(seen.opens.load(Ordering::SeqCst), 1);
    let session = first.session.expect("the session comes back");
    let layout = first.layout.expect("and the layout it reported");
    assert!(matches!(
        layout.pages,
        Some(PageLayout::Separate { count: Some(3), .. })
    ));
    let mut second = job(plugin, "stand");
    second.session = Some(session);
    second.page = 2;
    let again = landed(second.run(None));
    assert_eq!(
        seen.opens.load(Ordering::SeqCst),
        1,
        "the second page is asked of the open session"
    );
    assert_eq!(seen.views.load(Ordering::SeqCst), 2);
    assert!(again.session.is_some());
}

#[test]
fn a_view_that_is_not_a_picture_is_refused_and_the_session_still_comes_back() {
    let (plugin, _) = stand(Reply::Text, None);
    let l = landed(job(plugin, "stand").run(None));
    assert!(l.session.is_some());
    let f = l.outcome.err().expect("a refusal");
    assert_eq!(f.kind, FailureKind::Unsupported);
}

#[test]
fn pixels_that_do_not_add_up_make_no_picture() {
    let (plugin, _) = stand(Reply::BadPixels, None);
    assert!(image_payload(job(plugin, "stand").run(None)).is_none());
}

#[test]
fn an_svg_without_sharp_zoom_is_a_plain_picture_at_the_size_the_plugin_names() {
    let (plugin, _) = stand(Reply::PlainSvg, None);
    match image_payload(job(plugin, "stand").run(None)) {
        Some(MediaPayload::Static(img)) => {
            // 10x10 drawn at its own size or shrunk to the 64px the plugin named, never beyond.
            assert!(img.width() <= 64 && img.height() <= 64, "{}", img.width());
        }
        _ => panic!("a Static picture was expected"),
    }
}

#[test]
fn a_plugin_that_fails_to_open_lands_with_no_session() {
    struct NoOpen;
    impl Plugin for NoOpen {
        fn manifest(&self) -> Manifest {
            Manifest::new("no-open", "0")
        }
        fn open(&self, _: Input, _: &OpenCx) -> Result<Box<dyn Session>, Failure> {
            Err(Failure::corrupt(t!(en: "broken")))
        }
    }
    let l = landed(job(Arc::new(NoOpen), "stand").run(None));
    assert!(l.session.is_none() && l.layout.is_none());
    assert_eq!(l.outcome.err().unwrap().kind, FailureKind::Corrupt);
}

#[test]
fn a_failure_shows_its_reason_only_where_the_picture_has_one() {
    use crate::preview::image::ImageFailure;
    use crate::preview::svg_guard::SvgFail;
    let f = |k| Failure::new(k, t!(en: "x"));
    let of = |id: &str, k| image_failure_of(id, &f(k));
    // An SVG file that cannot be read names the reason the same way a failed drawing does.
    assert_eq!(
        of("svg", FailureKind::TooLarge),
        Some(ImageFailure::Svg(SvgFail::TooLarge))
    );
    assert_eq!(
        of("svg", FailureKind::Corrupt),
        Some(ImageFailure::Svg(SvgFail::Invalid))
    );
    assert_eq!(
        of("image", FailureKind::TooLarge),
        Some(ImageFailure::TooLarge)
    );
    assert_eq!(
        of("image", FailureKind::Unsupported),
        Some(ImageFailure::UnsupportedFormat)
    );
    assert_eq!(of("image", FailureKind::Io), Some(ImageFailure::Corrupt));
    // A cancelled job is not a failure to show.
    for id in ["svg", "image", "pdf", "video"] {
        assert_eq!(
            of(id, FailureKind::Cancelled),
            Some(ImageFailure::Cancelled),
            "{id}"
        );
    }
    // A PDF and a video have a fixed fallback text of their own.
    assert_eq!(of("pdf", FailureKind::Unsupported), None);
    assert_eq!(of("video", FailureKind::External), None);
}

#[test]
fn the_texts_of_the_plugins_failures_are_the_texts_the_host_shows() {
    use crate::i18n::{tr, Lang, Msg};
    let dir = unique_tmp("konoma_media_texts");
    std::fs::create_dir_all(&dir).unwrap();
    let fail = |plugin: &dyn Plugin, path: &Path| {
        let mut s = konoma_plugin::testing::open_file(plugin, path).unwrap();
        s.view(&ViewReq::default(), &ViewCx::bare()).err().unwrap()
    };
    let pdf = dir.join("bad.pdf");
    std::fs::write(&pdf, b"%PDF-1.4\nx").unwrap();
    let video = dir.join("bad.mp4");
    std::fs::write(&video, b"x").unwrap();
    let big = dir.join("big.svg");
    std::fs::File::create(&big)
        .unwrap()
        .set_len(33 << 20)
        .unwrap();
    let cases = [
        (
            fail(&konoma_media::PdfPlugin, &pdf),
            Msg::PdfPreviewUnavailable,
        ),
        (
            fail(&konoma_media::VideoPlugin, &video),
            Msg::VideoThumbUnavailable,
        ),
        (fail(&konoma_media::SvgPlugin, &big), Msg::SvgTooLarge),
        (
            fail(&konoma_media::SvgPlugin, &dir.join("missing.svg")),
            Msg::SvgInvalid,
        ),
    ];
    for (failure, msg) in cases {
        assert_eq!(failure.message.get("en"), tr(Lang::En, msg), "{msg:?} en");
        assert_eq!(failure.message.get("ja"), tr(Lang::Jp, msg), "{msg:?} ja");
    }
}

// ---- on the app -----------------------------------------------------------------------------

fn app_in(dir: &Path) -> App {
    let mut app = App::new(dir.to_path_buf(), Config::default()).unwrap();
    app.picker = Some(ratatui_image::picker::Picker::halfblocks());
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    Box::leak(Box::new(rx));
    app.img_tx = Some(tx);
    app
}

fn png(path: &Path, w: u32, h: u32) {
    image::RgbaImage::from_pixel(w, h, image::Rgba([10, 200, 30, 255]))
        .save(path)
        .unwrap();
}

fn recv(rx: &std::sync::mpsc::Receiver<MediaResult>) -> MediaResult {
    rx.recv_timeout(std::time::Duration::from_secs(30))
        .expect("the media worker must answer")
}

fn show(app: &mut App, kind: PreviewKind, path: &Path) {
    app.tab.preview_kind = Some(kind.clone());
    app.tab.preview_path = Some(path.to_path_buf());
    app.tab.mode = Mode::Preview;
    app.start_media_load(&kind, path);
}

#[test]
fn the_view_and_the_decode_run_in_one_go_on_the_worker_and_one_result_comes_back() {
    let dir = unique_tmp("konoma_media_one_go");
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("a.png");
    png(&p, 30, 20);
    let mut app = app_in(&dir);
    let (tx, rx) = std::sync::mpsc::channel();
    app.attach_media_loader(tx);
    show(&mut app, PreviewKind::image(p.clone()), &p);
    // Nothing is decoded on the UI thread; the load is under way.
    assert!(app.is_media_loading());
    assert!(app.image_src.is_none());
    // One result carries the plugin's session and the decoded picture: no second trip.
    let r = recv(&rx);
    assert!(app.apply_media(r));
    assert!(rx.try_recv().is_err(), "no second result follows");
    let src = app.image_src.as_ref().expect("the picture landed");
    assert_eq!((src.width(), src.height()), (30, 20));
    assert!(!app.is_media_loading());
    assert!(
        app.media_session.is_some(),
        "the session is kept for the next page"
    );
}

#[test]
fn the_view_of_a_picture_plugin_is_asked_on_the_worker_thread() {
    let (plugin, seen) = stand(Reply::Picture, None);
    let me = std::thread::current().id();
    let j = job(plugin, "stand");
    let worker = std::thread::spawn(move || {
        let _ = j.run(None);
    });
    worker.join().unwrap();
    let on = seen.view_thread.lock().unwrap().expect("viewed");
    assert_ne!(on, me);
}

#[test]
fn a_gif_is_told_from_a_still_by_its_first_bytes_and_animates() {
    let dir = unique_tmp("konoma_media_gif");
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("anim.gif");
    {
        use image::codecs::gif::GifEncoder;
        let file = std::fs::File::create(&p).unwrap();
        let mut enc = GifEncoder::new(file);
        for shade in [30u8, 120, 220] {
            let frame = image::Frame::new(image::RgbaImage::from_pixel(
                8,
                8,
                image::Rgba([shade, 0, 0, 255]),
            ));
            enc.encode_frame(frame).unwrap();
        }
    }
    let mut app = app_in(&dir);
    let (tx, rx) = std::sync::mpsc::channel();
    app.attach_media_loader(tx);
    show(&mut app, PreviewKind::image(p.clone()), &p);
    let r = recv(&rx);
    assert!(app.apply_media(r));
    assert!(app.is_gif_active(), "the frames of the animation are kept");
    assert!(app.image_src.is_none());
}

#[test]
fn an_image_that_cannot_be_decoded_records_why() {
    let dir = unique_tmp("konoma_media_corrupt");
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("broken.png");
    std::fs::write(&p, b"\x89PNG\r\n\x1a\nnot really").unwrap();
    let mut app = app_in(&dir);
    show(&mut app, PreviewKind::image(p.clone()), &p);
    assert!(app.image_src.is_none());
    assert!(app.image_failure().is_some(), "{:?}", app.image_failure());
}

#[test]
fn an_svg_keeps_its_source_for_sharp_zoom_and_is_drawn_untrusted() {
    let dir = unique_tmp("konoma_media_svg_app");
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("a.svg");
    std::fs::write(&p, SVG).unwrap();
    let mut app = app_in(&dir);
    show(&mut app, PreviewKind::svg(p.clone()), &p);
    assert!(app.image_src.is_some());
    assert!(
        app.vector_svg.is_some(),
        "the source is kept for sharp zoom"
    );
    assert!(app.image_logical.is_some());
}

#[test]
fn a_pdf_reports_its_page_count_when_it_lands_and_not_before() {
    let Some(sample) = sample_path_or_skip("sample.pdf") else {
        return;
    };
    let dir = unique_tmp("konoma_media_pdf_count");
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("doc.pdf");
    std::fs::copy(&sample, &p).unwrap();
    let mut app = app_in(&dir);
    let (tx, rx) = std::sync::mpsc::channel();
    app.attach_media_loader(tx);
    show(&mut app, PreviewKind::pdf(p.clone()), &p);
    // Counting the pages is the plugin's work, on the worker: until it lands the keys do nothing.
    assert_eq!(app.page_indicator(), None);
    assert!(!app.can_turn_pages());
    app.next_page();
    assert_eq!(app.tab.page, 1, "no page to turn to yet");
    let r = recv(&rx);
    assert!(app.apply_media(r));
    assert_eq!(app.page_indicator(), Some((1, 3)));
    assert!(app.can_turn_pages());
}

#[test]
fn holding_j_draws_only_the_newest_page_and_drops_the_older_results() {
    let Some(sample) = sample_path_or_skip("sample.pdf") else {
        return;
    };
    let dir = unique_tmp("konoma_media_pdf_mash");
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("doc.pdf");
    std::fs::copy(&sample, &p).unwrap();
    let mut app = app_in(&dir);
    let (tx, rx) = std::sync::mpsc::channel();
    app.attach_media_loader(tx);
    show(&mut app, PreviewKind::pdf(p.clone()), &p);
    let r = recv(&rx);
    assert!(app.apply_media(r));
    // Two page turns before either result is applied.
    app.next_page();
    app.next_page();
    assert_eq!(app.tab.page, 3);
    let (a, b) = (recv(&rx), recv(&rx));
    let mut verdicts = vec![app.apply_media(a), app.apply_media(b)];
    verdicts.sort();
    assert_eq!(
        verdicts,
        vec![false, true],
        "exactly one result is current: the older is dropped on arrival"
    );
    assert_eq!(app.page_indicator(), Some((3, 3)));
    assert!(app.image_src.is_some());
    assert!(!app.is_media_loading());
}

#[test]
fn a_page_turn_keeps_the_old_picture_on_screen_until_the_new_one_lands() {
    let Some(sample) = sample_path_or_skip("sample.pdf") else {
        return;
    };
    let dir = unique_tmp("konoma_media_pdf_keep");
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("doc.pdf");
    std::fs::copy(&sample, &p).unwrap();
    let mut app = app_in(&dir);
    let (tx, rx) = std::sync::mpsc::channel();
    app.attach_media_loader(tx);
    show(&mut app, PreviewKind::pdf(p.clone()), &p);
    let r = recv(&rx);
    assert!(app.apply_media(r));
    let before = app.image_src.clone().expect("page 1");
    app.next_page();
    assert!(app.is_media_loading());
    assert!(
        app.image_src
            .as_ref()
            .is_some_and(|s| Arc::ptr_eq(s, &before)),
        "page 1 stays until page 2 lands"
    );
    let r = recv(&rx);
    assert!(app.apply_media(r));
    assert!(app
        .image_src
        .as_ref()
        .is_some_and(|s| !Arc::ptr_eq(s, &before)));
}

#[test]
fn a_result_for_a_preview_that_moved_on_is_dropped_with_its_session() {
    let dir = unique_tmp("konoma_media_stale");
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("a.png");
    png(&p, 6, 6);
    let mut app = app_in(&dir);
    let (tx, rx) = std::sync::mpsc::channel();
    app.attach_media_loader(tx);
    show(&mut app, PreviewKind::image(p.clone()), &p);
    let r = recv(&rx);
    // The user moved on to another file before the result was applied.
    app.clear_image();
    assert!(!app.apply_media(r), "stale");
    assert!(app.image_src.is_none());
    assert!(app.media_session.is_none(), "its session is not kept");
}

#[test]
fn a_landing_for_another_kind_of_preview_is_not_adopted() {
    let (plugin, _) = stand(Reply::Picture, Some(3));
    let l = landed(job(plugin, "stand").run(None));
    let dir = unique_tmp("konoma_media_other_kind");
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = app_in(&dir);
    // The preview on screen is a text file: no picture plugin shows it.
    app.tab.preview_kind = Some(PreviewKind::Text(PathBuf::from("/x/a.txt")));
    app.apply_payload(MediaPayload::Plugin(l));
    assert!(app.media_session.is_none());
    assert!(app.image_src.is_none());
    assert_eq!(app.tab.page_count, None);
}

#[test]
fn leaving_the_picture_drops_its_session() {
    let dir = unique_tmp("konoma_media_leave");
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("a.png");
    png(&p, 6, 6);
    let mut app = app_in(&dir);
    show(&mut app, PreviewKind::image(p.clone()), &p);
    assert!(app.media_session.is_some());
    app.clear_image();
    assert!(app.media_session.is_none());
}

#[test]
fn a_session_of_another_file_is_not_reused() {
    let dir = unique_tmp("konoma_media_other_file");
    std::fs::create_dir_all(&dir).unwrap();
    let (a, b) = (dir.join("a.png"), dir.join("b.png"));
    png(&a, 6, 6);
    png(&b, 9, 9);
    let mut app = app_in(&dir);
    show(&mut app, PreviewKind::image(a.clone()), &a);
    let kept = app.media_session.as_ref().map(|s| s.path.clone());
    assert_eq!(kept.as_deref(), Some(a.as_path()));
    // Showing b without leaving a first (the preview kind changed under it): a's session is taken
    // out and not used for b.
    show(&mut app, PreviewKind::image(b.clone()), &b);
    assert_eq!(
        app.media_session
            .as_ref()
            .map(|s| s.path.clone())
            .as_deref(),
        Some(b.as_path())
    );
    assert_eq!(app.image_src.as_ref().map(|i| i.width()), Some(9));
}

#[test]
fn a_still_image_needs_the_decode_backend_and_the_others_only_a_picker() {
    let dir = unique_tmp("konoma_media_gates");
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("a.png");
    png(&p, 6, 6);
    let s = dir.join("a.svg");
    std::fs::write(&s, SVG).unwrap();
    // A picker but no decode backend: the SVG still draws, the still image does not.
    let mut app = App::new(dir.to_path_buf(), Config::default()).unwrap();
    app.picker = Some(ratatui_image::picker::Picker::halfblocks());
    show(&mut app, PreviewKind::image(p.clone()), &p);
    assert!(app.image_src.is_none());
    show(&mut app, PreviewKind::svg(s.clone()), &s);
    assert!(app.image_src.is_some());
    // No picker at all: nothing starts (the render side falls back to text).
    let mut bare = App::new(dir.to_path_buf(), Config::default()).unwrap();
    show(&mut bare, PreviewKind::svg(s.clone()), &s);
    assert!(bare.image_src.is_none());
    assert!(!bare.is_media_loading());
}

// ---- switching a plugin off, the old settings -----------------------------------------------

#[test]
fn a_plugin_switched_off_shows_nothing_and_its_files_fall_back_as_before() {
    let dir = unique_tmp("konoma_media_off");
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("a.png");
    png(&p, 6, 6);
    let mut cfg = Config::default();
    cfg.plugins.insert(
        "image".to_string(),
        toml::Table::from_iter([("enabled".to_string(), toml::Value::Boolean(false))]),
    );
    // The image file is no longer decided as a picture; being binary it is "can not preview".
    match cfg.resolve_preview(&p) {
        PreviewKind::CanNotPreview { ext } => assert_eq!(ext, "png"),
        other => panic!("{other:?}"),
    }
    // Entering the preview loads nothing.
    let mut app = App::new(dir.to_path_buf(), cfg).unwrap();
    app.picker = Some(ratatui_image::picker::Picker::halfblocks());
    app.enter_preview(&p);
    assert!(app.image_src.is_none());
    assert!(!app.is_media_loading());
    // Even a stale kind that still names the plugin starts nothing: it is off.
    show(&mut app, PreviewKind::image(p.clone()), &p);
    assert!(app.image_src.is_none());
    assert!(!app.is_media_loading());
}

#[test]
fn the_other_picture_plugins_are_not_affected_by_switching_one_off() {
    let dir = unique_tmp("konoma_media_off_svg");
    std::fs::create_dir_all(&dir).unwrap();
    let s = dir.join("a.svg");
    std::fs::write(&s, SVG).unwrap();
    let mut cfg = Config::default();
    cfg.plugins.insert(
        "image".to_string(),
        toml::Table::from_iter([("enabled".to_string(), toml::Value::Boolean(false))]),
    );
    assert!(cfg.resolve_preview(&s).is_plugin("svg"));
}

#[test]
fn the_svg_size_comes_from_the_old_ui_setting_unless_the_plugin_table_sets_it() {
    let mut cfg = Config::default();
    cfg.ui.svg_max_px = 321;
    assert_eq!(
        crate::plugins::effective_settings(&cfg, "svg").int("max_px", 0),
        321
    );
    cfg.plugins.insert(
        "svg".to_string(),
        toml::Table::from_iter([("max_px".to_string(), toml::Value::Integer(654))]),
    );
    assert_eq!(
        crate::plugins::effective_settings(&cfg, "svg").int("max_px", 0),
        654,
        "the new name wins"
    );
}

#[test]
fn a_plugin_failure_lands_as_the_reason_of_the_picture() {
    use crate::preview::image::ImageFailure;
    let (plugin, _) = stand(Reply::Fail(FailureKind::TooLarge), None);
    let out = job(plugin, "image").run(None);
    assert!(matches!(
        image_payload(out),
        Some(MediaPayload::ImageFailed(ImageFailure::TooLarge))
    ));
    // A PDF's failure has no reason of its own: the render side shows its fixed hint.
    let (plugin, _) = stand(Reply::Fail(FailureKind::Unsupported), None);
    assert!(image_payload(job(plugin, "pdf").run(None)).is_none());
}
