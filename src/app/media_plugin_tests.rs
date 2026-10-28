//! The picture plugins on the host side (`app/media_plugin.rs`): the work of the plugin and the
//! decoding happen in one go on the media worker, the session survives for the next page, a stale
//! result is dropped, the `external` switch and the other contexts reach the plugin, and a plugin
//! that is switched off shows nothing. The plugins here are stand-ins written for the test where a
//! property has to be observed on its own; the real `image`/`svg`/`pdf` plugins are used where the
//! property is about them.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use konoma_plugin::view::{ImageData, ImageView, Layout, PageLayout, View, ViewReq};
use konoma_plugin::{
    t, Failure, FailureKind, Input, Manifest, OpenCx, Plugin, Session, Settings, Value, ViewCx,
};

use super::plugin_host::{PluginResult, PluginWork};
use super::STAND_IN;
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
    /// The read scope the `view` context carried.
    scope: Mutex<Option<konoma_plugin::ReadScope>>,
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
    /// A picture file the host is asked to read.
    File(PathBuf),
    /// Another reply for each page (from 1; the last one repeats).
    Pages(Vec<Reply>),
}

struct Stand {
    seen: Arc<Seen>,
    reply: Reply,
    pages: Option<usize>,
    /// How long `open` takes.
    open_delay: std::time::Duration,
    /// How long each `view` takes (the page is slow to draw).
    view_delay: std::time::Duration,
    /// When set, `open` waits for this gate instead of sleeping `open_delay`: the open is then
    /// certainly still out whatever the speed of the machine (a sleep is a race with the host's
    /// first-frame wait).
    open_gate: Option<Arc<crate::plugins::test_plugins::Latch>>,
}

struct StandSession {
    seen: Arc<Seen>,
    reply: Reply,
    pages: Option<usize>,
    view_delay: std::time::Duration,
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
        match &self.open_gate {
            Some(gate) => gate.wait(),
            None => std::thread::sleep(self.open_delay),
        }
        Ok(Box::new(StandSession {
            seen: self.seen.clone(),
            reply: self.reply.clone(),
            pages: self.pages,
            view_delay: self.view_delay,
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
        *self.seen.scope.lock().unwrap() = Some(cx.read_scope.clone());
        *self.seen.view_thread.lock().unwrap() = Some(std::thread::current().id());
        self.seen.pages.lock().unwrap().push(req.page);
        std::thread::sleep(self.view_delay);
        let mut reply = &self.reply;
        if let Reply::Pages(each) = reply {
            reply = &each[req.page.unwrap_or(0).min(each.len() - 1)];
        }
        match reply {
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
            Reply::File(p) => Ok(View::Image(ImageView::new(ImageData::File(p.clone())))),
            Reply::Fail(kind) => Err(Failure::new(*kind, t!(en: "nope", ja: "だめ"))),
            Reply::Pages(_) => unreachable!("pages do not nest"),
        }
    }
}

const SVG: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><rect width="10" height="10"/></svg>"#;

fn stand(reply: Reply, pages: Option<usize>) -> (Arc<dyn Plugin>, Arc<Seen>) {
    stand_slow(reply, pages, Duration::ZERO, Duration::ZERO)
}

/// A stand-in whose `open` and `view` take the given time.
fn stand_slow(
    reply: Reply,
    pages: Option<usize>,
    open_delay: Duration,
    view_delay: Duration,
) -> (Arc<dyn Plugin>, Arc<Seen>) {
    let seen = Arc::new(Seen::default());
    (
        Arc::new(Stand {
            seen: seen.clone(),
            reply,
            pages,
            open_delay,
            view_delay,
            open_gate: None,
        }),
        seen,
    )
}

/// A stand-in whose `open` returns only when `gate` is opened.
fn stand_gated(
    reply: Reply,
    pages: Option<usize>,
    gate: &Arc<crate::plugins::test_plugins::Latch>,
) -> (Arc<dyn Plugin>, Arc<Seen>) {
    let seen = Arc::new(Seen::default());
    (
        Arc::new(Stand {
            seen: seen.clone(),
            reply,
            pages,
            open_delay: Duration::ZERO,
            view_delay: Duration::ZERO,
            open_gate: Some(Arc::clone(gate)),
        }),
        seen,
    )
}

fn job(plugin: Arc<dyn Plugin>, id: &str) -> PluginWork {
    let path = PathBuf::from("/x/file.bin");
    PluginWork {
        id: id.to_string(),
        gen: 0,
        job: PluginJob::Open {
            plugin,
            input: Input::File {
                path: path.clone(),
                hint: None,
            },
        },
        // The first page, counted from 0 in the request.
        req: ViewReq::new(80, 24).page(0),
        settings: Settings::new(),
        lang: "en".to_string(),
        pixels: false,
        colors: konoma_plugin::ColorDepth::default(),
        path,
        external: false,
        default_raster_px: 800,
        expects_picture: true,
        document: None,
        decode_cancel: None,
        on_layout: None,
        group: None,
        act_ids: Vec::new(),
        canvas_known: None,
        read_scope: konoma_plugin::ReadScope::unrestricted(),
    }
}

fn image_payload(result: PluginResult) -> Option<MediaPayload> {
    result.into_image_payload()
}

// ---- the job --------------------------------------------------------------------------------

#[test]
fn the_contexts_reach_the_plugin_in_open_and_view() {
    for external in [false, true] {
        let (plugin, seen) = stand(Reply::Picture, None);
        let mut j = job(plugin, "stand");
        j.external = external;
        j.pixels = true;
        j.lang = "ja".to_string();
        j.settings = Settings::new().with("k", Value::Int(7));
        let out = j.run_on_the_spot();
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
    j.req.page = Some(2);
    let _ = j.run_on_the_spot();
    assert_eq!(*seen.pages.lock().unwrap(), vec![Some(2)]);
}

#[test]
fn the_session_is_handed_back_and_a_later_job_reuses_it_without_opening_again() {
    let (plugin, seen) = stand(Reply::Picture, Some(3));
    let first = job(plugin.clone(), "stand").run_on_the_spot();
    assert_eq!(seen.opens.load(Ordering::SeqCst), 1);
    let session = first.session.expect("the session comes back");
    let layout = first.layout.expect("and the layout it reported");
    assert!(matches!(
        layout.pages,
        Some(PageLayout::Separate { count: Some(3), .. })
    ));
    let mut second = job(plugin, "stand");
    second.job = PluginJob::View { session };
    second.req.page = Some(1);
    let again = second.run_on_the_spot();
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
    let l = job(plugin, "stand").run_on_the_spot();
    assert!(l.session.is_some());
    let f = l.failure.expect("a refusal");
    assert_eq!(f.kind, FailureKind::Unsupported);
}

#[test]
fn pixels_that_do_not_add_up_make_no_picture() {
    let (plugin, _) = stand(Reply::BadPixels, None);
    assert!(image_payload(job(plugin, "stand").run_on_the_spot()).is_none());
}

#[test]
fn an_svg_without_sharp_zoom_is_a_plain_picture_at_the_size_the_plugin_names() {
    let (plugin, _) = stand(Reply::PlainSvg, None);
    match image_payload(job(plugin, "stand").run_on_the_spot()) {
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
    let l = job(Arc::new(NoOpen), "stand").run_on_the_spot();
    assert!(l.session.is_none() && l.layout.is_none());
    assert_eq!(l.failure.unwrap().kind, FailureKind::Corrupt);
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

/// The next final result of the worker. A layout sent ahead of a picture (the UI stopped waiting
/// for it) is applied on the way, as the run loop would, and is not what is returned.
#[track_caller]
fn recv(app: &mut App, rx: &std::sync::mpsc::Receiver<MediaResult>) -> MediaResult {
    loop {
        let r = recv_any(rx);
        if matches!(r.payload, Some(MediaPayload::PluginLayout(_))) {
            app.apply_media(r);
            continue;
        }
        return r;
    }
}

/// The next message of the worker, a layout included.
#[track_caller]
fn recv_any(rx: &std::sync::mpsc::Receiver<MediaResult>) -> MediaResult {
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
    let r = recv(&mut app, &rx);
    assert!(app.apply_media(r));
    assert!(rx.try_recv().is_err(), "no second result follows");
    let src = app.image_src.as_ref().expect("the picture landed");
    assert_eq!((src.width(), src.height()), (30, 20));
    assert!(!app.is_media_loading());
    assert!(
        app.plugin.as_ref().is_some_and(|p| p.session.is_some()),
        "the session is kept for the next page"
    );
}

#[test]
fn the_view_of_a_picture_plugin_is_asked_on_the_worker_thread() {
    let (plugin, seen) = stand(Reply::Picture, None);
    let me = std::thread::current().id();
    let j = job(plugin, "stand");
    let worker = std::thread::spawn(move || {
        let _ = j.run_on_the_spot();
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
    let r = recv(&mut app, &rx);
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
fn the_raw_text_notice_of_a_scope_refusal_names_the_plugin_in_both_languages() {
    // `svg` shows its file as text with the reason (`RawText { with_reason: true }`): the reason
    // of a read-scope refusal carries the plugin's id, never the raw `<id>` placeholder.
    let dir = unique_tmp("konoma_media_rawtext_reason");
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("a.svg");
    std::fs::write(&p, "<svg/>").unwrap();
    for lang in [crate::i18n::Lang::En, crate::i18n::Lang::Jp] {
        let mut app = app_in(&dir);
        app.lang = lang;
        app.image_failure = Some(crate::preview::image::ImageFailure::OutsideReadDirs);
        let (body, _) = crate::ui::preview::unavailable_body(&app, "svg", &p);
        assert!(!body.contains("<id>"), "{body}");
        assert!(
            body.contains("[plugins.svg]") && body.contains("read_dirs"),
            "{body}"
        );
    }
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

/// The page count is what `open` learns, so it reaches the host before the first page is drawn:
/// the keys that turn pages work as soon as the PDF is opened (it used to be known only when the
/// first page had been drawn).
#[test]
fn a_pdf_reports_its_page_count_before_the_first_page_is_drawn() {
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
    // Within the first frame's wait, or else as the first message of the worker (the layout goes
    // ahead of the picture): the picture is not drawn yet either way.
    if app.page_indicator().is_none() {
        let r = recv_any(&rx);
        assert!(matches!(r.payload, Some(MediaPayload::PluginLayout(_))));
        assert!(app.apply_media(r));
    }
    assert_eq!(app.page_indicator(), Some((1, 3)));
    assert!(app.can_turn_pages());
    assert!(app.image_src.is_none(), "page 1 is not on screen yet");
    assert!(app.is_media_loading(), "the picture is still being made");
    // Then the picture lands and nothing changes about the count.
    let r = recv(&mut app, &rx);
    assert!(app.apply_media(r));
    assert!(app.image_src.is_some());
    assert_eq!(app.page_indicator(), Some((1, 3)));
}

/// `J` right after the file is opened, while page 1 is still being drawn, asks the plugin for
/// page 2 (the guard against the page keys being dead until the first picture lands).
#[test]
fn j_right_after_opening_asks_for_page_two_before_page_one_is_drawn() {
    let dir = unique_tmp("konoma_media_j_early");
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("doc.pdf");
    std::fs::write(&p, b"x").unwrap();
    let mut app = app_in(&dir);
    let (tx, rx) = std::sync::mpsc::channel();
    app.attach_media_loader(tx);
    app.tab.preview_kind = Some(PreviewKind::pdf(p.clone()));
    app.tab.preview_path = Some(p.clone());
    app.tab.mode = Mode::Preview;
    // `open` is quick and learns three pages; every page takes 400ms to draw.
    let (plugin, seen) = stand_slow(
        Reply::Picture,
        Some(3),
        Duration::ZERO,
        Duration::from_millis(400),
    );
    // Every start on this thread (the first open and the page turn) gets the stand-in.
    STAND_IN.with(|s| *s.borrow_mut() = Some(plugin));
    app.start_media_plugin(konoma_media::PDF_ID, &p);
    if app.page_indicator().is_none() {
        let r = recv_any(&rx);
        assert!(app.apply_media(r), "the layout message");
    }
    assert_eq!(app.page_indicator(), Some((1, 3)));
    assert!(app.image_src.is_none(), "page 1 is still being drawn");
    app.next_page(); // J
    assert_eq!(app.tab.page, 2, "the key worked before any page was drawn");
    // Results until the newest one lands: page 1's is stale and dropped.
    while app.is_media_loading() {
        let r = recv_any(&rx);
        app.apply_media(r);
    }
    assert_eq!(app.page_indicator(), Some((2, 3)));
    assert!(app.image_src.is_some());
    let asked = seen.pages.lock().unwrap().clone();
    assert!(
        asked.contains(&Some(1)),
        "the plugin was asked for page 2 (index 1): {asked:?}"
    );
}

/// A plugin whose `open` takes longer than the first frame waits still gets its page count to the
/// host ahead of the picture, as a message of its own that does not end the loading.
#[test]
fn a_slow_open_sends_the_layout_ahead_of_the_picture() {
    let dir = unique_tmp("konoma_media_layout_msg");
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("doc.pdf");
    std::fs::write(&p, b"x").unwrap();
    let mut app = app_in(&dir);
    let (tx, rx) = std::sync::mpsc::channel();
    app.attach_media_loader(tx);
    app.tab.preview_kind = Some(PreviewKind::pdf(p.clone()));
    app.tab.preview_path = Some(p.clone());
    app.tab.mode = Mode::Preview;
    let gate = Arc::new(crate::plugins::test_plugins::Latch::default());
    let (plugin, _) = stand_gated(Reply::Picture, Some(3), &gate);
    app.start_media_plugin_with(plugin, konoma_media::PDF_ID, &p);
    // The call returned while `open` is held at the gate: the UI did not wait for it.
    assert!(!gate.is_open());
    assert_eq!(app.page_indicator(), None, "open has not returned yet");
    gate.open();
    let first = recv_any(&rx);
    assert!(
        matches!(first.payload, Some(MediaPayload::PluginLayout(_))),
        "the layout comes first"
    );
    assert!(app.apply_media(first));
    assert_eq!(app.page_indicator(), Some((1, 3)));
    assert!(app.is_media_loading(), "a layout does not end the loading");
    assert!(app.image_src.is_none());
    let second = recv_any(&rx);
    assert!(matches!(second.payload, Some(MediaPayload::Plugin(_))));
    assert!(app.apply_media(second));
    assert!(!app.is_media_loading());
    assert!(app.image_src.is_some());
}

/// A layout that arrives for a generation that has moved on is dropped and ends nothing.
#[test]
fn a_stale_layout_message_is_dropped() {
    let dir = unique_tmp("konoma_media_layout_stale");
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("doc.pdf");
    std::fs::write(&p, b"x").unwrap();
    let mut app = app_in(&dir);
    let (tx, rx) = std::sync::mpsc::channel();
    app.attach_media_loader(tx);
    app.tab.preview_kind = Some(PreviewKind::pdf(p.clone()));
    app.tab.preview_path = Some(p.clone());
    app.tab.mode = Mode::Preview;
    let gate = Arc::new(crate::plugins::test_plugins::Latch::default());
    let (plugin, _) = stand_gated(Reply::Picture, Some(3), &gate);
    app.start_media_plugin_with(plugin, konoma_media::PDF_ID, &p);
    // Held at the gate past the host's wait: the layout comes through the channel.
    gate.open();
    let layout = recv_any(&rx);
    assert!(matches!(
        layout.payload,
        Some(MediaPayload::PluginLayout(_))
    ));
    // The user moved on before it was applied.
    app.clear_image();
    assert!(!app.apply_media(layout));
    assert_eq!(app.tab.page_count, None);
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
    let r = recv(&mut app, &rx);
    assert!(app.apply_media(r));
    // Two page turns before either result is applied.
    app.next_page();
    app.next_page();
    assert_eq!(app.tab.page, 3);
    let (a, b) = (recv(&mut app, &rx), recv(&mut app, &rx));
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
    let r = recv(&mut app, &rx);
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
    let r = recv(&mut app, &rx);
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
    let r = recv(&mut app, &rx);
    // The user moved on to another file before the result was applied.
    app.clear_image();
    assert!(!app.apply_media(r), "stale");
    assert!(app.image_src.is_none());
    assert!(app.plugin.is_none(), "its session is not kept");
}

#[test]
fn a_landing_for_another_kind_of_preview_is_not_adopted() {
    let (plugin, _) = stand(Reply::Picture, Some(3));
    let l = job(plugin, "stand").run_on_the_spot();
    let dir = unique_tmp("konoma_media_other_kind");
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = app_in(&dir);
    // The preview on screen is a text file: no picture plugin shows it.
    app.tab.preview_kind = Some(PreviewKind::Text(PathBuf::from("/x/a.txt")));
    app.apply_payload(MediaPayload::Plugin(Box::new(l)));
    assert!(app.plugin.is_none());
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
    assert!(app.plugin.as_ref().is_some_and(|p| p.session.is_some()));
    app.clear_image();
    assert!(app.plugin.is_none());
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
    let kept = app.plugin.as_ref().map(|s| s.path.clone());
    assert_eq!(kept.as_deref(), Some(a.as_path()));
    // Showing b without leaving a first (the preview kind changed under it): a's session is taken
    // out and not used for b.
    show(&mut app, PreviewKind::image(b.clone()), &b);
    assert_eq!(
        app.plugin.as_ref().map(|s| s.path.clone()).as_deref(),
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
    let out = job(plugin, "image").run_on_the_spot();
    assert!(matches!(
        image_payload(out),
        Some(MediaPayload::ImageFailed(ImageFailure::TooLarge))
    ));
    // A PDF's failure has no reason of its own: the render side shows its fixed hint.
    let (plugin, _) = stand(Reply::Fail(FailureKind::Unsupported), None);
    assert!(image_payload(job(plugin, "pdf").run_on_the_spot()).is_none());
}

// ---- a picture plugin's `ImageData::File` goes through the one door -------------------------------

fn file_view(shown: &Path, p: &Path) -> Option<MediaPayload> {
    decode_image_view(
        ImageView::new(ImageData::File(p.to_path_buf())),
        shown,
        100,
        None,
        false,
        &konoma_plugin::ReadScope::unrestricted(),
    )
}

/// Run `f` on a thread and fail (not hang) when it does not finish: an open that blocks on a FIFO.
fn within_ten_seconds<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(f());
    });
    rx.recv_timeout(Duration::from_secs(10))
        .expect("a special file was opened (the decode is stuck)")
}

fn failed(p: &Option<MediaPayload>) -> bool {
    matches!(p, Some(MediaPayload::ImageFailed(_)))
}

#[test]
fn a_fifo_is_refused_by_the_picture_view_without_blocking() {
    let dir = unique_tmp("konoma_pic_fifo");
    std::fs::create_dir_all(&dir).unwrap();
    let fifo = dir.join("pipe.png");
    if !std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .is_ok_and(|s| s.success())
    {
        return;
    }
    let shown = dir.join("a.spr");
    let (f, s) = (fifo.to_path_buf(), shown.clone());
    assert!(failed(&within_ten_seconds(move || file_view(&s, &f))));
    // The relative spelling of the same FIFO.
    let s = shown.clone();
    assert!(failed(&within_ten_seconds(move || file_view(
        &s,
        Path::new("pipe.png")
    ))));
    // The previewed file itself being a FIFO: the host's own GIF sniff does not open it either.
    let f = fifo.to_path_buf();
    assert!(!within_ten_seconds(move || App::looks_like_gif(&f)));
}

#[test]
fn a_device_and_a_directory_are_refused_by_the_picture_view() {
    let dir = unique_tmp("konoma_pic_dev");
    std::fs::create_dir_all(&dir).unwrap();
    let shown = dir.join("a.spr");
    for special in [
        Path::new("/dev/null"),
        Path::new("/dev/zero"),
        dir.as_path(),
    ] {
        let (s, p) = (shown.clone(), special.to_path_buf());
        assert!(
            failed(&within_ten_seconds(move || file_view(&s, &p))),
            "{special:?} is not a picture file"
        );
    }
    assert!(!App::looks_like_gif(Path::new("/dev/null")));
    assert!(!App::looks_like_gif(&dir));
}

#[test]
fn a_relative_picture_path_is_read_from_the_previewed_files_folder() {
    let dir = unique_tmp("konoma_pic_rel");
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    png(&dir.join("sub").join("pic.png"), 5, 3);
    let shown = dir.join("sub").join("a.spr");
    // Not the folder konoma runs in: the same name there does not exist.
    match file_view(&shown, Path::new("pic.png")) {
        Some(MediaPayload::Static(img)) => assert_eq!((img.width(), img.height()), (5, 3)),
        _ => panic!("the picture next to the previewed file is shown"),
    }
    // Absolute stays absolute.
    assert!(matches!(
        file_view(&shown, &dir.join("sub").join("pic.png")),
        Some(MediaPayload::Static(_))
    ));
    // A relative path that names the previewed file itself is not joined onto its folder twice.
    let rel_shown = Path::new("konoma_pic_rel_never_exists/x.png");
    assert!(failed(&file_view(rel_shown, rel_shown)));
}

#[test]
fn a_gif_file_of_the_picture_view_is_an_animation_read_from_the_opened_handle() {
    use image::codecs::gif::GifEncoder;
    let dir = unique_tmp("konoma_pic_gif");
    std::fs::create_dir_all(&dir).unwrap();
    {
        let mut enc = GifEncoder::new(std::fs::File::create(dir.join("anim.gif")).unwrap());
        for c in [10u8, 200] {
            enc.encode_frame(image::Frame::new(image::RgbaImage::from_pixel(
                4,
                4,
                image::Rgba([c, 0, 0, 255]),
            )))
            .unwrap();
        }
    }
    let shown = dir.join("a.spr");
    match file_view(&shown, Path::new("anim.gif")) {
        Some(MediaPayload::Gif(frames)) => assert_eq!(frames.len(), 2),
        _ => panic!("two frames animate"),
    }
}

#[test]
fn a_tga_the_content_cannot_identify_is_decoded_by_its_extension_like_before() {
    // A TGA has no signature: `ImageReader::open` took its format from the extension, and the
    // handle the host reads from must too (a user rule can send `*.tga` to the image plugin).
    let dir = unique_tmp("konoma_pic_tga");
    std::fs::create_dir_all(&dir).unwrap();
    let mut bytes = Vec::new();
    image::DynamicImage::new_rgba8(3, 2)
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Tga,
        )
        .unwrap();
    let tga = dir.join("x.tga");
    std::fs::write(&tga, &bytes).unwrap();
    let shown = dir.join("shown.tga");
    match file_view(&shown, &tga) {
        Some(MediaPayload::Static(img)) => assert_eq!((img.width(), img.height()), (3, 2)),
        other => panic!("a TGA by its extension is a still: {:?}", other.is_some()),
    }
    // The extension is only the fallback: the same bytes under a name that says nothing are not
    // guessed, and a PNG under a `.tga` name is still a PNG (the content wins).
    let nameless = dir.join("x.bin");
    std::fs::write(&nameless, &bytes).unwrap();
    assert!(failed(&file_view(&shown, &nameless)));
    let mut png = Vec::new();
    image::DynamicImage::new_rgba8(4, 4)
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();
    let liar = dir.join("liar.tga");
    std::fs::write(&liar, &png).unwrap();
    match file_view(&shown, &liar) {
        Some(MediaPayload::Static(img)) => assert_eq!(img.width(), 4),
        _ => panic!("the content wins over the extension"),
    }
}

// ---- the read scope of a plugin the user added ----------------------------------------------------

mod read_scope {
    use super::*;
    use konoma_plugin::ReadScope;

    fn scope_of(dirs: &[&Path]) -> ReadScope {
        ReadScope::within(dirs.iter().copied())
    }

    fn ok(p: &Option<MediaPayload>) -> bool {
        matches!(p, Some(MediaPayload::Static(_)))
    }

    fn refused(p: &Option<MediaPayload>) -> bool {
        matches!(
            p,
            Some(MediaPayload::ImageFailed(
                crate::preview::image::ImageFailure::OutsideReadDirs
            ))
        )
    }

    fn view(scope: &ReadScope, shown: &Path, p: &Path) -> Option<MediaPayload> {
        decode_image_view(
            ImageView::new(ImageData::File(p.to_path_buf())),
            shown,
            100,
            None,
            false,
            scope,
        )
    }

    /// root / preview dir / elsewhere, each with one picture.
    struct Tree {
        _dir: crate::test_support::TmpDir,
        root: PathBuf,
        preview_dir: PathBuf,
        elsewhere: PathBuf,
    }

    fn tree(name: &str) -> Tree {
        let dir = unique_tmp(name);
        let root = dir.join("root");
        let preview_dir = dir.join("preview");
        let elsewhere = dir.join("elsewhere");
        for d in [&root, &preview_dir, &elsewhere] {
            std::fs::create_dir_all(d).unwrap();
            png(&d.join("pic.png"), 4, 3);
        }
        std::fs::write(preview_dir.join("a.spr"), b"x").unwrap();
        Tree {
            root: std::fs::canonicalize(&root).unwrap(),
            preview_dir: std::fs::canonicalize(&preview_dir).unwrap(),
            elsewhere: std::fs::canonicalize(&elsewhere).unwrap(),
            _dir: dir,
        }
    }

    #[test]
    fn a_file_inside_the_root_is_read() {
        let t = tree("rs_root");
        let scope = scope_of(&[&t.root, &t.preview_dir]);
        let shown = t.preview_dir.join("a.spr");
        assert!(ok(&view(&scope, &shown, &t.root.join("pic.png"))));
    }

    #[test]
    fn a_file_in_the_preview_folder_outside_the_root_is_read() {
        let t = tree("rs_preview");
        let scope = scope_of(&[&t.root, &t.preview_dir]);
        let shown = t.preview_dir.join("a.spr");
        assert!(ok(&view(&scope, &shown, &t.preview_dir.join("pic.png"))));
        // The relative spelling lands in the same folder.
        assert!(ok(&view(&scope, &shown, Path::new("pic.png"))));
    }

    #[test]
    fn a_file_outside_both_is_refused_and_the_reason_is_the_scope() {
        let t = tree("rs_outside");
        let scope = scope_of(&[&t.root, &t.preview_dir]);
        let shown = t.preview_dir.join("a.spr");
        assert!(refused(&view(&scope, &shown, &t.elsewhere.join("pic.png"))));
    }

    #[test]
    fn read_dirs_widens_the_scope() {
        let t = tree("rs_widen");
        let shown = t.preview_dir.join("a.spr");
        let narrow = scope_of(&[&t.root, &t.preview_dir]);
        let wide = scope_of(&[&t.root, &t.preview_dir, &t.elsewhere]);
        let pic = t.elsewhere.join("pic.png");
        assert!(refused(&view(&narrow, &shown, &pic)));
        assert!(ok(&view(&wide, &shown, &pic)));
    }

    #[test]
    fn the_previewed_file_itself_is_always_readable() {
        // A plugin that hands back exactly the file being previewed works, even when that file
        // is in none of the folders of the scope (a file opened by `konoma <file>` elsewhere).
        let t = tree("rs_self");
        let shown = t.elsewhere.join("pic.png");
        let scope = scope_of(&[&t.root]);
        assert!(ok(&view(&scope, &shown, &shown)));
        // Spelled relative to itself too.
        assert!(ok(&view(&scope, &shown, Path::new("pic.png"))));
        // A symbolic link to the previewed file is the same file.
        let link = t.root.join("link.png");
        std::os::unix::fs::symlink(&shown, &link).unwrap();
        assert!(ok(&view(&scope, &shown, &link)));
    }

    #[test]
    fn the_previewed_file_works_for_a_whole_job() {
        // End to end through the worker job: a user plugin whose `view` returns the previewed
        // file as its picture.
        let t = tree("rs_job");
        let shown = t.elsewhere.join("pic.png");
        let (plugin, seen) = stand(Reply::File(shown.clone()), None);
        let mut j = job(plugin, "stand");
        j.path = shown.clone();
        j.read_scope = scope_of(&[&t.root]);
        let got = image_payload(j.run_on_the_spot());
        assert!(ok(&got));
        // The plugin was told the scope it works under.
        let told = seen.scope.lock().unwrap().clone().expect("a view context");
        assert_eq!(told.dirs(), Some(&[t.root.clone()][..]));
    }

    #[test]
    fn a_whole_job_refuses_a_file_outside_the_scope() {
        let t = tree("rs_job_out");
        let (plugin, _) = stand(Reply::File(t.elsewhere.join("pic.png")), None);
        let mut j = job(plugin, "stand");
        j.path = t.preview_dir.join("a.spr");
        j.read_scope = scope_of(&[&t.root, &t.preview_dir]);
        assert!(refused(&image_payload(j.run_on_the_spot())));
    }

    #[test]
    fn the_prefix_trap_a_sibling_with_the_same_start_is_outside() {
        let dir = unique_tmp("rs_prefix");
        let b = dir.join("b");
        let bc = dir.join("bc");
        std::fs::create_dir_all(&b).unwrap();
        std::fs::create_dir_all(&bc).unwrap();
        png(&bc.join("pic.png"), 2, 2);
        let scope = scope_of(&[&b]);
        let shown = b.join("a.spr");
        assert!(refused(&view(&scope, &shown, &bc.join("pic.png"))));
    }

    #[test]
    fn dot_dot_cannot_climb_out() {
        let t = tree("rs_dotdot");
        let scope = scope_of(&[&t.root]);
        let shown = t.root.join("a.spr");
        let climbing = t.root.join("..").join("elsewhere").join("pic.png");
        assert!(refused(&view(&scope, &shown, &climbing)));
        // A relative spelling climbs the same way.
        let rel = Path::new("../elsewhere/pic.png");
        assert!(refused(&view(&scope, &shown, rel)));
        // A `..` that stays inside is fine.
        std::fs::create_dir_all(t.root.join("sub")).unwrap();
        let inside = t.root.join("sub").join("..").join("pic.png");
        assert!(ok(&view(&scope, &shown, &inside)));
    }

    #[test]
    fn a_link_inside_that_points_out_is_refused() {
        let t = tree("rs_link_out");
        let scope = scope_of(&[&t.root]);
        let link = t.root.join("out.png");
        std::os::unix::fs::symlink(t.elsewhere.join("pic.png"), &link).unwrap();
        let shown = t.root.join("a.spr");
        assert!(refused(&view(&scope, &shown, &link)));
        // A linked folder too.
        let dir_link = t.root.join("dirlink");
        std::os::unix::fs::symlink(&t.elsewhere, &dir_link).unwrap();
        assert!(refused(&view(&scope, &shown, &dir_link.join("pic.png"))));
    }

    #[test]
    fn a_link_outside_that_points_in_is_allowed() {
        let t = tree("rs_link_in");
        let scope = scope_of(&[&t.root]);
        let link = t.elsewhere.join("in.png");
        std::os::unix::fs::symlink(t.root.join("pic.png"), &link).unwrap();
        let shown = t.root.join("a.spr");
        assert!(ok(&view(&scope, &shown, &link)));
    }

    #[test]
    fn a_link_that_leads_nowhere_is_not_a_scope_matter() {
        let t = tree("rs_dangling");
        let scope = scope_of(&[&t.root]);
        let link = t.root.join("gone.png");
        std::os::unix::fs::symlink(t.root.join("nothing.png"), &link).unwrap();
        let shown = t.root.join("a.spr");
        // Cannot be read, but it is not blamed on the scope.
        assert!(!refused(&view(&scope, &shown, &link)));
        assert!(failed(&view(&scope, &shown, &link)));
    }

    #[test]
    fn a_standard_plugin_reads_anywhere() {
        // The scope a standard plugin gets is no scope: Markdown and the image plugin read
        // absolute paths anywhere, as they always did.
        let t = tree("rs_standard");
        let shown = t.root.join("a.md");
        let free = ReadScope::unrestricted();
        assert!(ok(&view(&free, &shown, &t.elsewhere.join("pic.png"))));
        let scope = crate::plugins::read_scope::scope_for(true, None, &t.root, &shown, None);
        assert!(!scope.is_restricted());
        assert!(ok(&view(&scope, &shown, &t.elsewhere.join("pic.png"))));
    }

    fn svg_view(base: &Path, scope: &ReadScope, svg: &str) -> Option<MediaPayload> {
        decode_image_view(
            ImageView {
                data: ImageData::Svg {
                    data: svg.as_bytes().into(),
                    base_dir: Some(base.to_path_buf()),
                    trusted: false,
                },
                sharp_zoom: true,
                raster_max_px: Some(64),
            },
            &base.join("a.spr"),
            64,
            None,
            false,
            scope,
        )
    }

    #[test]
    fn an_svg_naming_a_folder_outside_is_refused_with_the_reason() {
        // The folder an SVG reads its references from is a path the plugin gave: outside the
        // scope the picture is refused (the same reason as any other picture outside), not
        // silently drawn without its files; inside it the SVG is drawn and redrawn within the scope.
        let t = tree("rs_svg_base");
        let scope = scope_of(&[&t.root]);
        assert!(refused(&svg_view(&t.elsewhere, &scope, SVG)));
        match svg_view(&t.root, &scope, SVG) {
            Some(MediaPayload::Vector { trust, .. }) => {
                assert!(matches!(trust, SvgTrust::FileWithin(..)), "kept in scope");
            }
            _ => panic!("the SVG is drawn"),
        }
        // A folder that does not exist is not a scope matter.
        let gone = t.root.join("gone");
        assert!(!refused(&svg_view(&gone, &scope, SVG)));
        // No limit: the old behaviour (the plain file trust).
        match svg_view(&t.elsewhere, &ReadScope::unrestricted(), SVG) {
            Some(MediaPayload::Vector { trust, .. }) => {
                assert!(matches!(trust, SvgTrust::File(_)));
            }
            _ => panic!("the SVG is drawn"),
        }
    }

    fn red_png(p: &Path) {
        image::RgbaImage::from_pixel(20, 20, image::Rgba([255, 0, 0, 255]))
            .save(p)
            .unwrap();
    }

    fn svg_image(href: &Path) -> String {
        format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="20" height="20"><image xlink:href="{}" width="20" height="20"/></svg>"#,
            href.display()
        )
    }

    fn centre_alpha(p: &Option<MediaPayload>) -> u8 {
        let img = match p {
            Some(MediaPayload::Vector { img, .. }) => img.to_rgba8(),
            Some(MediaPayload::Static(img)) => img.to_rgba8(),
            _ => panic!("the SVG is drawn"),
        };
        img.get_pixel(img.width() / 2, img.height() / 2)[3]
    }

    #[test]
    fn drawing_a_retained_svg_again_still_refuses_a_file_outside_the_scope() {
        // Zoom / resize re-rasterise through `SvgTrust::draw`: the `FileWithin` arm must judge
        // every href against the roots, like the first drawing, and `File` must not.
        let t = tree("rs_svg_redraw");
        let secret = t.elsewhere.join("secret.png");
        red_png(&secret);
        let inside = t.root.join("ok.png");
        red_png(&inside);
        let alpha = |img: Option<image::DynamicImage>| {
            let img = img.expect("the SVG is drawn").to_rgba8();
            img.get_pixel(img.width() / 2, img.height() / 2)[3]
        };
        let base = t.root.join("a.svg");
        let within = SvgTrust::FileWithin(base.clone(), vec![t.root.clone()].into());
        let never = || false;
        assert_eq!(
            alpha(within.draw(svg_image(&secret).as_bytes(), 64, &never)),
            0,
            "outside href refused on redraw"
        );
        assert_eq!(
            alpha(within.draw(svg_image(&inside).as_bytes(), 64, &never)),
            255,
            "inside href still drawn on redraw"
        );
        // The plain file trust has no scope and reads it (so the test tells the arms apart).
        assert_eq!(
            alpha(SvgTrust::File(base).draw(svg_image(&secret).as_bytes(), 64, &never)),
            255
        );
    }

    #[test]
    fn an_svg_preview_through_a_link_out_of_the_root_is_not_refused() {
        // The previewed file is a link to a file outside the root; the plugin names the real
        // parent as the SVG's folder. The default scope holds that folder.
        let t = tree("rs_svg_link");
        let real = t.elsewhere.join("t.svg");
        std::fs::write(&real, SVG).unwrap();
        let link = t.root.join("l.svg");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let scope = crate::plugins::read_scope::scope_for(false, None, &t.root, &link, None);
        assert!(!refused(&svg_view(&t.elsewhere, &scope, SVG)));
        match svg_view(&t.elsewhere, &scope, SVG) {
            Some(MediaPayload::Vector { .. }) => {}
            _ => panic!("the SVG is drawn"),
        }
        // A folder that is neither the link's nor its target's is still refused.
        assert!(refused(&svg_view(&t.preview_dir, &scope, SVG)));
    }

    #[test]
    fn a_file_named_inside_an_svg_is_read_only_within_the_scope() {
        // Scenario (a): the plugin hands the SVG bytes; the href inside names an absolute file
        // outside the scope. It is not read (the picture comes out empty). An href inside is.
        let t = tree("rs_svg_href");
        let scope = scope_of(&[&t.root]);
        let secret = t.elsewhere.join("secret.png");
        red_png(&secret);
        let inside = t.root.join("ok.png");
        red_png(&inside);
        assert_eq!(
            centre_alpha(&svg_view(&t.root, &scope, &svg_image(&secret))),
            0
        );
        assert_eq!(
            centre_alpha(&svg_view(&t.root, &scope, &svg_image(&inside))),
            255,
            "an href inside the scope still draws"
        );
        // A relative href climbing out of the scope is judged the same way.
        std::fs::create_dir_all(t.root.join("sub")).unwrap();
        let climbing = svg_image(Path::new("../../elsewhere/secret.png"));
        assert_eq!(
            centre_alpha(&svg_view(&t.root.join("sub"), &scope, &climbing)),
            0
        );
        // Without a limit the same SVG reads the file (the test is about the scope).
        assert_eq!(
            centre_alpha(&svg_view(
                &t.root,
                &ReadScope::unrestricted(),
                &svg_image(&secret)
            )),
            255
        );
    }

    #[test]
    fn the_refusal_message_names_the_key_in_both_languages() {
        use crate::i18n::Lang;
        use crate::preview::image::{ImageFailure, ImageFailureMessage};
        for lang in [Lang::En, Lang::Jp] {
            let why = ImageFailure::OutsideReadDirs.message(lang, "demo").unwrap();
            assert!(why.contains("read_dirs"), "{why}");
        }
        assert_ne!(
            ImageFailure::OutsideReadDirs.message(Lang::En, "demo"),
            ImageFailure::OutsideReadDirs.message(Lang::Jp, "demo")
        );
    }

    #[test]
    fn the_refusal_is_not_taken_for_a_damaged_file() {
        // A distinct reason: the user is told what to change.
        let t = tree("rs_distinct");
        let scope = scope_of(&[&t.root]);
        let shown = t.root.join("a.spr");
        let got = view(&scope, &shown, &t.elsewhere.join("pic.png"));
        assert!(!matches!(
            got,
            Some(MediaPayload::ImageFailed(
                crate::preview::image::ImageFailure::Corrupt
            ))
        ));
    }

    #[test]
    fn a_file_in_scope_still_obeys_the_regular_file_rules() {
        // The judged path is opened with the same rules: a FIFO inside the scope is refused
        // without blocking.
        let t = tree("rs_fifo");
        let fifo = t.root.join("pipe.png");
        if !std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .is_ok_and(|s| s.success())
        {
            return;
        }
        let scope = scope_of(&[&t.root]);
        let shown = t.root.join("a.spr");
        let s = scope.clone();
        let got = within_ten_seconds(move || view(&s, &shown, &fifo));
        assert!(failed(&got) && !refused(&got));
    }
}

// ---- the mapping from the shared decoding (`decode_picture_view`) to the media payload --------

mod payload_mapping {
    use super::super::payload_of;
    use super::*;
    use crate::preview::image::ImageFailure;
    use konoma_plugin::canvas_pictures::PictureView;

    fn img(w: u32) -> image::DynamicImage {
        image::DynamicImage::ImageRgba8(image::RgbaImage::new(w, 1))
    }

    #[test]
    fn a_still_is_a_static_payload() {
        match payload_of(PictureView::Still(img(7))) {
            MediaPayload::Static(i) => assert_eq!(i.width(), 7),
            _ => panic!("a still is Static"),
        }
    }

    #[test]
    fn a_vector_keeps_its_svg_and_its_trust() {
        let svg = Arc::new(b"<svg/>".to_vec());
        let trust = SvgTrust::File(PathBuf::from("/x/a.svg"));
        match payload_of(PictureView::Vector {
            img: img(3),
            svg: svg.clone(),
            trust: trust.clone(),
        }) {
            MediaPayload::Vector {
                img: i,
                svg: s,
                trust: t,
            } => {
                assert_eq!(i.width(), 3);
                assert!(Arc::ptr_eq(&s, &svg));
                assert_eq!(t, trust);
            }
            _ => panic!("a vector is Vector"),
        }
    }

    #[test]
    fn frames_are_a_gif_payload_in_order_with_their_delays() {
        let frames = vec![
            (img(1), Duration::from_millis(30)),
            (img(2), Duration::from_millis(70)),
        ];
        match payload_of(PictureView::Gif(frames)) {
            MediaPayload::Gif(f) => {
                let got: Vec<(u32, Duration)> = f.iter().map(|(i, d)| (i.width(), *d)).collect();
                assert_eq!(
                    got,
                    vec![
                        (1, Duration::from_millis(30)),
                        (2, Duration::from_millis(70))
                    ]
                );
            }
            _ => panic!("frames are Gif"),
        }
    }

    #[test]
    fn every_failure_reason_is_carried_unchanged() {
        use crate::preview::svg_guard::SvgFail;
        let all = [
            ImageFailure::TooLarge,
            ImageFailure::Corrupt,
            ImageFailure::UnsupportedFormat,
            ImageFailure::Cancelled,
            ImageFailure::Svg(SvgFail::TooLarge),
            ImageFailure::Svg(SvgFail::Invalid),
            ImageFailure::OutsideReadDirs,
        ];
        // Distinct reasons stay distinct (a stable code each), none is turned into another.
        let codes: std::collections::HashSet<_> = all.iter().map(|f| f.code()).collect();
        assert_eq!(codes.len(), all.len());
        for why in all {
            match payload_of(PictureView::Failed(why)) {
                MediaPayload::ImageFailed(got) => assert_eq!(got, why),
                _ => panic!("a failure is ImageFailed"),
            }
        }
    }

    #[test]
    fn the_decoding_through_the_host_maps_each_kind() {
        let shown = Path::new("/x/a.svg");
        let scope = konoma_plugin::ReadScope::unrestricted();
        let rgba = ImageView::new(ImageData::Rgba {
            width: 2,
            height: 1,
            pixels: Arc::new(vec![0; 8]),
        });
        assert!(matches!(
            decode_image_view(rgba, shown, 100, None, false, &scope),
            Some(MediaPayload::Static(_))
        ));
        let lazy_kind = ImageView::new(ImageData::Frames(vec![]));
        assert!(matches!(
            decode_image_view(lazy_kind, shown, 100, None, false, &scope),
            Some(MediaPayload::ImageFailed(ImageFailure::UnsupportedFormat))
        ));
        let bad = ImageView::new(ImageData::Rgba {
            width: 9,
            height: 9,
            pixels: Arc::new(vec![0; 4]),
        });
        assert!(decode_image_view(bad, shown, 100, None, false, &scope).is_none());
    }
}

// ---- turning from an animation to another kind of page -----------------------------------------

/// A document of pages: page 1 is a two-frame animation, then `next`. Returns the app (on page 1,
/// its animation playing) and the worker's receiver.
fn gif_page_then(
    name: &str,
    next: Reply,
) -> (
    App,
    std::sync::mpsc::Receiver<MediaResult>,
    crate::test_support::TmpDir,
) {
    let dir = unique_tmp(name);
    std::fs::create_dir_all(&dir).unwrap();
    let gif = dir.join("anim.gif");
    std::fs::write(
        &gif,
        konoma_test_support::build_animated_gif(8, 8, &[([255, 0, 0], 600_000), ([0, 0, 255], 20)]),
    )
    .unwrap();
    let still = dir.join("still.png");
    png(&still, 8, 8);
    let p = dir.join("doc.png");
    std::fs::write(&p, b"x").unwrap();
    let mut app = app_in(&dir);
    let (tx, rx) = std::sync::mpsc::channel();
    app.attach_media_loader(tx);
    app.tab.preview_kind = Some(PreviewKind::image(p.clone()));
    app.tab.preview_path = Some(p.clone());
    app.tab.mode = Mode::Preview;
    let next = match next {
        Reply::File(_) => Reply::File(still),
        other => other,
    };
    let (plugin, _seen) = stand(Reply::Pages(vec![Reply::File(gif), next]), Some(2));
    STAND_IN.with(|s| *s.borrow_mut() = Some(plugin));
    app.start_media_plugin("image", &p);
    while app.is_media_loading() || !app.is_gif_active() {
        let r = recv_any(&rx);
        app.apply_media(r);
    }
    assert!(app.is_gif_active(), "page 1 is an animation");
    (app, rx, dir)
}

fn settle_turn(app: &mut App, rx: &std::sync::mpsc::Receiver<MediaResult>) {
    while app.is_media_loading() {
        let r = recv_any(rx);
        app.apply_media(r);
    }
}

#[test]
fn turning_from_an_animation_to_a_still_page_shows_the_still_and_stops_the_animation() {
    let (mut app, rx, _dir) = gif_page_then("konoma_gif_to_still", Reply::File(PathBuf::new()));
    app.next_page();
    assert!(
        app.is_gif_active(),
        "the animation stays on screen while the next page is drawn"
    );
    settle_turn(&mut app, &rx);
    assert!(app.image_src.is_some(), "the still page landed");
    assert!(
        !app.is_gif_active(),
        "the animation of the page before is gone once a still lands"
    );
    assert!(app.gif_protocol().is_none());
}

#[test]
fn turning_from_an_animation_to_a_failing_page_says_why_and_stops_the_animation() {
    let (mut app, rx, _dir) =
        gif_page_then("konoma_gif_to_fail", Reply::Fail(FailureKind::Corrupt));
    app.next_page();
    assert!(app.is_gif_active(), "held while the next page is drawn");
    settle_turn(&mut app, &rx);
    assert!(app.image_failure().is_some(), "the failure is recorded");
    assert!(
        !app.is_gif_active(),
        "a page that fails does not leave the animation of the page before"
    );
}

#[test]
fn a_dropped_stale_answer_does_not_stop_the_animation() {
    let (mut app, _rx, _dir) = gif_page_then("konoma_gif_cancelled", Reply::File(PathBuf::new()));
    app.apply_payload(MediaPayload::ImageFailed(
        crate::preview::image::ImageFailure::Cancelled,
    ));
    assert!(app.is_gif_active(), "a cancelled load says nothing");
}

// ---- a failure that lands lets go of the picture of the page before ------------------------------

/// The screen text of the preview, drawn as the host does.
fn preview_text(app: &mut App) -> String {
    let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 12)).unwrap();
    term.draw(|f| crate::ui::preview::render(f, app, f.area()))
        .unwrap();
    let buf = term.backend().buffer();
    (0..buf.area.height)
        .map(|y| {
            (0..buf.area.width)
                .map(|x| buf[(x, y)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A document of two pages for the plugin `id` (a file named `name`): page 1 is a picture that has
/// landed, page 2 is `second`. Returns the app on page 1.
fn picture_then(
    tag: &str,
    id: &str,
    name: &str,
    first: Reply,
    second: Reply,
) -> (
    App,
    std::sync::mpsc::Receiver<MediaResult>,
    crate::test_support::TmpDir,
) {
    let dir = unique_tmp(tag);
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join(name);
    std::fs::write(&p, b"x").unwrap();
    let mut app = app_in(&dir);
    let (tx, rx) = std::sync::mpsc::channel();
    app.attach_media_loader(tx);
    app.tab.preview_kind = Some(PreviewKind::plugin(id, p.clone()));
    app.tab.preview_path = Some(p.clone());
    app.tab.mode = Mode::Preview;
    let (plugin, _seen) = stand(Reply::Pages(vec![first, second]), Some(2));
    STAND_IN.with(|s| *s.borrow_mut() = Some(plugin));
    app.start_media_plugin(id, &p);
    while app.is_media_loading() {
        let r = recv_any(&rx);
        app.apply_media(r);
    }
    assert!(app.image_src.is_some(), "page 1 is on screen");
    (app, rx, dir)
}

/// A non-picture plugin (a PDF): its page that fails to draw does not leave the page before on
/// screen; the host's own words for the plugin come instead.
#[test]
fn a_pdf_page_that_fails_lets_go_of_the_page_before_and_says_the_pdf_words() {
    let (mut app, rx, _dir) = picture_then(
        "konoma_pdf_fail_release",
        konoma_media::PDF_ID,
        "doc.pdf",
        Reply::Picture,
        Reply::Fail(FailureKind::Corrupt),
    );
    app.next_page();
    assert!(app.image_src.is_some(), "held while the next page is drawn");
    settle_turn(&mut app, &rx);
    assert!(app.plugin_failure().is_some(), "the failure landed");
    assert!(app.image_src.is_none(), "the old page is let go");
    assert!(!app.is_image_preview());
    let text = preview_text(&mut app);
    assert!(text.contains("[pdf] cannot render"), "{text}");
    assert!(text.contains("doc.pdf"), "{text}");
}

/// A picture plugin: the page that fails after a still page shows the reason, not the still.
#[test]
fn an_image_page_that_fails_after_a_still_page_shows_the_reason_and_not_the_still() {
    let (mut app, rx, _dir) = picture_then(
        "konoma_img_fail_release",
        konoma_media::IMAGE_ID,
        "doc.png",
        Reply::Picture,
        Reply::Fail(FailureKind::Corrupt),
    );
    app.next_page();
    assert!(app.image_src.is_some(), "held while the next page is drawn");
    settle_turn(&mut app, &rx);
    assert!(app.image_failure().is_some());
    assert!(app.image_src.is_none(), "the old picture is let go");
    let text = preview_text(&mut app);
    assert!(text.contains("[image]"), "{text}");
}

/// A stale answer that was dropped (`Cancelled`) says nothing: the picture stays.
#[test]
fn a_cancelled_page_answer_keeps_the_picture_of_the_page_before() {
    let (mut app, rx, _dir) = picture_then(
        "konoma_img_cancel_keep",
        konoma_media::IMAGE_ID,
        "doc.png",
        Reply::Picture,
        Reply::Fail(FailureKind::Cancelled),
    );
    app.next_page();
    settle_turn(&mut app, &rx);
    assert!(
        app.image_src.is_some(),
        "a dropped answer lets go of nothing"
    );
    assert!(app.image_failure().is_none());
}
