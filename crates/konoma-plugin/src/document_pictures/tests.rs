//! The document-picture rules that every host shares. Nothing here reaches the network (the
//! download is a stand-in `fn`) or the real cache directory (a sandboxed root is set first), and
//! ordering is made with channels, never with a sleep.

use std::cell::RefCell;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};

use image::DynamicImage;
use konoma_svg::image::ImageFailure;
use konoma_test_support::unique_tmp;
use konoma_widgets::inline_images::{DecodedImage, InlineImageCache, MdImgEntry, SvgTrust};
use konoma_widgets::markdown::{math_url, mermaid_fence_url, ImageSlot, MathSlot, MermaidSlot};

use super::*;
use crate::canvas_pictures::{EmbedPicture, SvgDraw};

/// The state a host keeps, owned by the test.
struct State {
    cache: InlineImageCache,
    inflight: HashSet<String>,
    failed: HashSet<String>,
    results: Option<Sender<DecodedImage>>,
    fetches: Option<Sender<RemoteFetch>>,
    remote_images: bool,
    fetcher: Fetcher,
}

impl State {
    fn new() -> State {
        State {
            cache: InlineImageCache::default(),
            inflight: HashSet::new(),
            failed: HashSet::new(),
            results: None,
            fetches: None,
            remote_images: true,
            fetcher: |_, _| false,
        }
    }

    fn with_results(mut self) -> (State, Receiver<DecodedImage>) {
        let (tx, rx) = channel();
        self.results = Some(tx);
        (self, rx)
    }

    fn with_fetches(mut self) -> (State, Receiver<RemoteFetch>) {
        let (tx, rx) = channel();
        self.fetches = Some(tx);
        (self, rx)
    }

    fn pictures(&mut self) -> DocPictures<'_> {
        DocPictures {
            cache: &mut self.cache,
            remote_inflight: &mut self.inflight,
            remote_failed: &mut self.failed,
            results: self.results.clone(),
            fetches: self.fetches.clone(),
            remote_images: self.remote_images,
            fetcher: self.fetcher,
        }
    }
}

/// A render that waits for a token on `gate` before it answers (a failed render: no pixels
/// needed), so a test decides when a worker finishes.
fn gated(gate: &Arc<Mutex<Receiver<()>>>) -> impl FnOnce() -> EmbedPicture + Send + 'static {
    let gate = gate.clone();
    move || {
        let _ = gate.lock().unwrap().recv();
        (Err("stand-in render".to_string()), None)
    }
}

fn fence_key(n: usize) -> PathBuf {
    PathBuf::from(mermaid_fence_url(&format!("graph TD; A{n}-->B")))
}

fn png(path: &Path, w: u32, h: u32) {
    image::RgbaImage::new(w, h)
        .save_with_format(path, image::ImageFormat::Png)
        .unwrap();
}

#[test]
fn the_synthetic_key_is_the_same_for_the_same_source() {
    let mut st = State::new();
    // No results channel: rendered here, so the key is visible in the cache at once.
    let code = "graph TD; A-->B";
    assert_eq!(
        st.pictures()
            .request_fence(code.into(), None, "fence:mermaid", 512),
        Requested::Done
    );
    let key = PathBuf::from(mermaid_fence_url(code));
    assert_eq!(st.cache.cache.keys().collect::<Vec<_>>(), vec![&key]);
    // The same source is cached: nothing renders again, and no second entry appears.
    assert_eq!(
        st.pictures()
            .request_fence(code.into(), None, "fence:mermaid", 512),
        Requested::Nothing
    );
    assert_eq!(st.cache.cache.len(), 1);
    // Another source, or the same expression in the other display mode, is another key.
    assert_ne!(
        mermaid_fence_url(code),
        mermaid_fence_url("graph TD; A-->C")
    );
    assert_ne!(math_url("x^2", false), math_url("x^2", true));
    assert_eq!(math_url("x^2", true), math_url("x^2", true));
    st.pictures()
        .request_math("x^2".into(), true, None, "math", 512);
    st.pictures()
        .request_math("x^2".into(), false, None, "math", 512);
    assert_eq!(st.cache.cache.len(), 3);
}

#[test]
fn a_synchronous_render_without_a_plugin_fails_the_picture() {
    // No plugin takes the kind (switched off): the same "cannot be drawn" as a failed render, so
    // the page shows the text fallback instead of waiting.
    let mut st = State::new();
    st.pictures()
        .request_math("a+b".into(), false, None, "math", 512);
    let e = &st.cache.cache[&PathBuf::from(math_url("a+b", false))];
    assert!(e.failed);
}

#[test]
fn the_in_flight_cap_is_respected_and_a_freed_slot_is_reused() {
    let (mut st, rx) = State::new().with_results();
    let (gate_tx, gate_rx) = channel();
    let gate = Arc::new(Mutex::new(gate_rx));
    let total = MAX_SYNTHETIC_RENDERS_IN_FLIGHT + 4;
    let mut started = 0;
    for n in 0..total {
        let r = st.pictures().request_rendered(
            fence_key(n),
            SvgTrust::Embedded,
            "mermaid",
            gated(&gate),
        );
        if r == Requested::Started {
            started += 1;
        } else {
            assert_eq!(r, Requested::Nothing);
        }
    }
    assert_eq!(started, MAX_SYNTHETIC_RENDERS_IN_FLIGHT);
    assert_eq!(
        st.pictures().synthetic_in_flight(),
        MAX_SYNTHETIC_RENDERS_IN_FLIGHT
    );
    // The ones over the cap left no placeholder behind: the next pass asks for them again.
    assert_eq!(st.cache.cache.len(), MAX_SYNTHETIC_RENDERS_IN_FLIGHT);
    // Let one worker finish, file its result: a slot is free again.
    gate_tx.send(()).unwrap();
    let res = rx.recv().unwrap();
    let applied = st.pictures().apply_result(res);
    assert!(applied.redraw && applied.relayout);
    assert_eq!(
        st.pictures().synthetic_in_flight(),
        MAX_SYNTHETIC_RENDERS_IN_FLIGHT - 1
    );
    let again = st.pictures().request_rendered(
        fence_key(total),
        SvgTrust::Embedded,
        "mermaid",
        gated(&gate),
    );
    assert_eq!(again, Requested::Started);
    // Release the rest so no worker outlives the test.
    for _ in 0..MAX_SYNTHETIC_RENDERS_IN_FLIGHT {
        gate_tx.send(()).unwrap();
    }
    for _ in 0..MAX_SYNTHETIC_RENDERS_IN_FLIGHT {
        rx.recv().unwrap();
    }
}

#[test]
fn a_panicking_render_still_answers() {
    let (mut st, rx) = State::new().with_results();
    st.pictures().request_rendered(
        fence_key(0),
        SvgTrust::Embedded,
        "mermaid",
        || -> EmbedPicture { panic!("stand-in panic") },
    );
    let res = rx.recv().unwrap();
    assert!(res.image.is_err());
    assert!(st.pictures().apply_result(res).redraw);
    assert!(st.cache.cache[&fence_key(0)].failed);
}

#[test]
fn the_sweep_removes_only_the_pictures_that_left_the_document() {
    let mut st = State::new();
    let (keep_fence, gone_fence) = (fence_key(1), fence_key(2));
    let keep_math = PathBuf::from(math_url("x", false));
    let gone_math = PathBuf::from(math_url("y", true));
    let plain = PathBuf::from("/tmp/some/image.png");
    for k in [&keep_fence, &gone_fence, &keep_math, &gone_math, &plain] {
        st.cache.cache.insert(k.clone(), MdImgEntry::default());
    }
    let live = live_synthetic_keys(
        &["graph TD; A1-->B".to_string()],
        &[("x".to_string(), false)],
    );
    assert!(live.contains(&keep_fence) && live.contains(&keep_math));
    st.pictures().sweep(&live);
    let mut left: Vec<&PathBuf> = st.cache.cache.keys().collect();
    left.sort();
    let mut want = vec![&keep_fence, &keep_math, &plain];
    want.sort();
    assert_eq!(
        left, want,
        "a picture that is not a fence or an expression is never swept"
    );
}

#[test]
fn a_file_outside_the_read_scope_is_refused_without_being_opened() {
    let (mut st, rx) = State::new().with_results();
    let dir = unique_tmp("doc_pictures_scope");
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("a.png");
    png(&file, 8, 8);
    st.cache.cache.insert(file.clone(), MdImgEntry::default());
    let job = DecodeJob {
        svg_max_px: 256,
        roots: None,
        // The host judged the file outside the plugin's folders: no real path to read.
        read_path: None,
        svg: Arc::new(NoSvg),
        generation: (0, Arc::new(AtomicU64::new(0))),
    };
    assert!(st.pictures().spawn_decode(file.clone(), job));
    let res = rx.recv().unwrap();
    assert_eq!(
        res.image.as_ref().unwrap_err(),
        ImageFailure::OutsideReadDirs.code()
    );
    // The refusal is filed like any failure: the picture is text, not "loading".
    let applied = st.pictures().apply_result(res);
    assert!(applied.relayout);
    assert!(st.cache.cache[&file].failed);
}

struct NoSvg;
impl SvgDraw for NoSvg {
    fn untrusted(
        &self,
        _: &[u8],
        _: &Path,
        _: Option<&[PathBuf]>,
        _: u32,
        _: &dyn Fn() -> bool,
    ) -> Result<DynamicImage, konoma_svg::svg::RasterError> {
        Err(konoma_svg::svg::RasterError::Failed(
            konoma_svg::svg_guard::SvgFail::Invalid,
        ))
    }
    fn embedded(
        &self,
        _: &[u8],
        _: u32,
        _: &dyn Fn() -> bool,
    ) -> Result<DynamicImage, konoma_svg::svg::RasterError> {
        Err(konoma_svg::svg::RasterError::Failed(
            konoma_svg::svg_guard::SvgFail::Invalid,
        ))
    }
}

#[test]
fn a_file_inside_the_scope_is_decoded_and_filed() {
    let (mut st, rx) = State::new().with_results();
    let dir = unique_tmp("doc_pictures_decode");
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("a.png");
    png(&file, 8, 4);
    st.cache.cache.insert(file.clone(), MdImgEntry::default());
    let job = DecodeJob {
        svg_max_px: 256,
        roots: None,
        read_path: Some(file.clone()),
        svg: Arc::new(NoSvg),
        generation: (0, Arc::new(AtomicU64::new(0))),
    };
    assert!(st.pictures().spawn_decode(file.clone(), job));
    let res = rx.recv().unwrap();
    assert!(res.image.is_ok());
    let applied = st.pictures().apply_result(res);
    assert!(applied.redraw && applied.pixels);
    // An ordinary picture's size is known from its file: filing the pixels does not re-lay out.
    assert!(!applied.relayout);
    assert!(st.cache.cache[&file].decoded.is_some());
}

#[test]
fn a_result_of_an_earlier_request_is_discarded() {
    let mut st = State::new();
    let key = fence_key(7);
    st.cache.cache.insert(key.clone(), MdImgEntry::default());
    let old = st.cache.next_request(&key);
    let new = st.cache.next_request(&key);
    assert_ne!(old, new);
    let stale = DecodedImage {
        path: key.clone(),
        image: Ok(DynamicImage::new_rgba8(4, 4)),
        svg: None,
        reraster: false,
        frames: None,
        request: old,
    };
    let applied = st.pictures().apply_result(stale);
    assert_eq!(
        applied,
        Applied {
            redraw: false,
            relayout: false,
            pixels: false
        }
    );
    let e = &st.cache.cache[&key];
    assert!(
        e.decoded.is_none() && !e.failed,
        "a stale result touches nothing"
    );
}

#[test]
fn a_decode_whose_preview_moved_on_is_reported_cancelled_and_forgotten() {
    let (mut st, rx) = State::new().with_results();
    let dir = unique_tmp("doc_pictures_moved_on");
    std::fs::create_dir_all(&dir).unwrap();
    // Not an image, so the decode fails: with the generation changed that failure is "cancelled".
    let file = dir.join("broken.png");
    std::fs::write(&file, b"not an image").unwrap();
    st.cache.cache.insert(file.clone(), MdImgEntry::default());
    let latest = Arc::new(AtomicU64::new(1));
    let job = DecodeJob {
        svg_max_px: 256,
        roots: None,
        read_path: Some(file.clone()),
        svg: Arc::new(NoSvg),
        // Started under generation 0; the host is at 1 by now.
        generation: (0, latest.clone()),
    };
    assert!(st.pictures().spawn_decode(file.clone(), job));
    let res = rx.recv().unwrap();
    assert_eq!(
        res.image.as_ref().unwrap_err(),
        ImageFailure::Cancelled.code()
    );
    // Filing it forgets the entry (nobody wants it), so the picture is asked for again if its
    // document comes back, instead of staying failed.
    let applied = st.pictures().apply_result(res);
    assert!(!applied.redraw);
    assert!(!st.cache.cache.contains_key(&file));
    assert_eq!(latest.load(Ordering::Relaxed), 1);
}

#[test]
fn nothing_is_fetched_when_remote_images_are_off() {
    static CALLS: AtomicUsize = AtomicUsize::new(0);
    let (mut st, rx) = State::new().with_fetches();
    st.remote_images = false;
    st.fetcher = |_, _| {
        CALLS.fetch_add(1, Ordering::SeqCst);
        true
    };
    let url = "https://example.com/never.png";
    // The first time the URL is marked failed, and the host is told to rebuild its layout (the
    // placeholder it just drew says "loading"); after that there is nothing to tell.
    assert!(st.pictures().request_remote(url));
    assert!(!st.pictures().request_remote(url));
    assert!(st.failed.contains(url));
    assert!(st.inflight.is_empty());
    assert!(rx.try_recv().is_err());
    assert_eq!(CALLS.load(Ordering::SeqCst), 0);
    // A local path is not a remote image at all.
    assert!(!st.pictures().request_remote("./local.png"));
}

#[test]
fn a_remote_image_is_fetched_once_and_a_failure_is_not_retried() {
    static CALLS: AtomicUsize = AtomicUsize::new(0);
    let root = unique_tmp("doc_pictures_cache_root");
    std::fs::create_dir_all(&root).unwrap();
    set_test_cache_root(root.to_path_buf());
    let (mut st, rx) = State::new().with_fetches();
    st.fetcher = |_, _| {
        CALLS.fetch_add(1, Ordering::SeqCst);
        false
    };
    let url = "https://example.com/a.png";
    assert!(!st.pictures().request_remote(url));
    assert!(st.inflight.contains(url));
    // Already downloading: no second thread.
    assert!(!st.pictures().request_remote(url));
    let res = rx.recv().unwrap();
    assert_eq!((res.url.as_str(), res.ok), (url, false));
    st.pictures().apply_fetch(res);
    assert!(st.inflight.is_empty() && st.failed.contains(url));
    // Failed: not asked for again.
    assert!(!st.pictures().request_remote(url));
    assert_eq!(CALLS.load(Ordering::SeqCst), 1);
    assert!(rx.try_recv().is_err());
}

#[test]
fn a_panicking_download_is_reported_as_a_failure() {
    let root = unique_tmp("doc_pictures_cache_root_panic");
    std::fs::create_dir_all(&root).unwrap();
    set_test_cache_root(root.to_path_buf());
    let (mut st, rx) = State::new().with_fetches();
    st.fetcher = |_, _| panic!("stand-in panic");
    let url = "https://example.com/p.png";
    st.pictures().request_remote(url);
    let res = rx.recv().unwrap();
    assert!(!res.ok);
    st.pictures().apply_fetch(res);
    assert!(st.inflight.is_empty(), "the URL must not stay in flight");
}

#[test]
fn the_cache_path_of_a_remote_image_is_stable_and_follows_the_root() {
    let root = unique_tmp("doc_pictures_cache_path");
    set_test_cache_root(root.to_path_buf());
    let a = md_remote_cache_path("https://example.com/a.png").unwrap();
    assert_eq!(
        a,
        md_remote_cache_path(" https://example.com/a.png ").unwrap()
    );
    assert_ne!(
        a,
        md_remote_cache_path("https://example.com/b.png").unwrap()
    );
    assert!(a.starts_with(root.join("konoma").join("remote-images")));
    // A downloaded file is found as a local one.
    assert!(resolve_md_image_path("https://example.com/a.png", None).is_none());
    std::fs::create_dir_all(a.parent().unwrap()).unwrap();
    png(&a, 2, 2);
    assert_eq!(
        resolve_md_image_path("https://example.com/a.png", None),
        Some(a)
    );
}

// ---- Slots ----

struct Fixture {
    st: State,
    refusals: RefCell<Vec<String>>,
}

impl Fixture {
    fn new() -> Fixture {
        Fixture {
            st: State::new(),
            refusals: RefCell::new(Vec::new()),
        }
    }

    /// Looks up with a 8x16 px cell and 80 columns; `refused` names the plugin a file is refused
    /// for.
    fn lookup<R>(
        &self,
        font: Option<(u16, u16)>,
        base_dir: Option<PathBuf>,
        refused: &dyn Fn(&str, &Path) -> Option<String>,
        f: impl FnOnce(&PictureLookup<'_>) -> R,
    ) -> R {
        let message = |f: ImageFailure, pid: &str| Some(format!("{}@{pid}", f.code()));
        let other = |_: &str, _: Option<u16>| None;
        let l = PictureLookup {
            cache: &self.st.cache,
            remote_failed: &self.st.failed,
            font,
            avail: 80,
            base_dir,
            fence_target_rows: 20,
            mermaid_on: true,
            math_on: true,
            plugin_id: "doc",
            refused,
            refusals: &self.refusals,
            message: &message,
            other: &other,
        };
        f(&l)
    }
}

fn never(_: &str, _: &Path) -> Option<String> {
    None
}

#[test]
fn an_image_slot_is_inline_loading_or_unavailable() {
    let dir = unique_tmp("doc_pictures_slots");
    std::fs::create_dir_all(&dir).unwrap();
    png(&dir.join("ok.png"), 80, 160);
    let mut fx = Fixture::new();
    let base = Some(dir.to_path_buf());
    let font = Some((8, 16));
    // A file that exists: its size in cells.
    let slot = fx.lookup(font, base.clone(), &never, |l| l.image("ok.png", None));
    assert!(matches!(slot, ImageSlot::Inline { cols, rows } if cols > 0 && rows > 0));
    // A narrower budget (a table cell) gives a narrower picture.
    let wide = fx.lookup(font, base.clone(), &never, |l| l.image("ok.png", None));
    let narrow = fx.lookup(font, base.clone(), &never, |l| l.image("ok.png", Some(2)));
    let (ImageSlot::Inline { cols: w, .. }, ImageSlot::Inline { cols: n, .. }) = (wide, narrow)
    else {
        panic!("both inline");
    };
    assert!(n <= 2 && n < w);
    // A missing file, and a data: URL, are text.
    assert_eq!(
        fx.lookup(font, base.clone(), &never, |l| l.image("missing.png", None)),
        ImageSlot::Unavailable
    );
    assert_eq!(
        fx.lookup(font, base.clone(), &never, |l| l
            .image("data:image/png;base64,AAAA", None)),
        ImageSlot::Unavailable
    );
    // No graphics backend: everything is text.
    assert_eq!(
        fx.lookup(None, base.clone(), &never, |l| l.image("ok.png", None)),
        ImageSlot::Unavailable
    );
    // A remote image that is not here yet is loading, until its download has failed.
    let url = "https://example.com/not-cached-anywhere-0001.png";
    let root = unique_tmp("doc_pictures_slots_root");
    set_test_cache_root(root.to_path_buf());
    assert_eq!(
        fx.lookup(font, base.clone(), &never, |l| l.image(url, None)),
        ImageSlot::Loading
    );
    fx.st.failed.insert(url.to_string());
    assert_eq!(
        fx.lookup(font, base, &never, |l| l.image(url, None)),
        ImageSlot::Unavailable
    );
}

#[test]
fn a_refused_or_failed_picture_says_why_and_a_refusal_is_recorded() {
    let dir = unique_tmp("doc_pictures_slots_refused");
    std::fs::create_dir_all(&dir).unwrap();
    png(&dir.join("ok.png"), 16, 16);
    let mut fx = Fixture::new();
    let base = Some(dir.to_path_buf());
    let refuse = |_: &str, _: &Path| Some("other-plugin".to_string());
    let slot = fx.lookup(Some((8, 16)), base.clone(), &refuse, |l| {
        l.image("ok.png", None)
    });
    assert_eq!(
        slot,
        ImageSlot::Failed(format!(
            "{}@other-plugin",
            ImageFailure::OutsideReadDirs.code()
        ))
    );
    assert_eq!(*fx.refusals.borrow(), vec!["other-plugin".to_string()]);
    // A picture whose decode failed keeps its reason.
    let file = dir.join("ok.png");
    fx.st.cache.cache.insert(
        file,
        MdImgEntry {
            failed: true,
            fail: Some(ImageFailure::Corrupt),
            ..MdImgEntry::default()
        },
    );
    let slot = fx.lookup(Some((8, 16)), base, &never, |l| l.image("ok.png", None));
    assert_eq!(
        slot,
        ImageSlot::Failed(format!("{}@doc", ImageFailure::Corrupt.code()))
    );
}

#[test]
fn fence_and_math_slots_follow_the_cache() {
    let mut fx = Fixture::new();
    let font = Some((8, 16));
    // The probe asks whether fences are pictures at all.
    assert_eq!(
        fx.lookup(font, None, &never, |l| l.mermaid("")),
        MermaidSlot::Loading
    );
    // Nothing yet: loading. No font: text.
    assert_eq!(
        fx.lookup(font, None, &never, |l| l.mermaid("a")),
        MermaidSlot::Loading
    );
    assert_eq!(
        fx.lookup(None, None, &never, |l| l.mermaid("a")),
        MermaidSlot::Text
    );
    assert_eq!(
        fx.lookup(font, None, &never, |l| l.math("x", false)),
        MathSlot::Loading
    );
    assert_eq!(
        fx.lookup(None, None, &never, |l| l.math("x", false)),
        MathSlot::Raw
    );
    // Sized: the reserved size comes from `layout_px`.
    let fk = PathBuf::from(mermaid_fence_url("a"));
    let mk = PathBuf::from(math_url("x", false));
    fx.st.cache.cache.insert(
        fk.clone(),
        MdImgEntry {
            layout_px: Some((400, 200)),
            ..MdImgEntry::default()
        },
    );
    fx.st.cache.cache.insert(
        mk.clone(),
        MdImgEntry {
            layout_px: Some((200, 40)),
            ..MdImgEntry::default()
        },
    );
    assert!(matches!(
        fx.lookup(font, None, &never, |l| l.mermaid("a")),
        MermaidSlot::Image { cols, rows } if cols > 0 && rows > 0
    ));
    assert!(matches!(
        fx.lookup(font, None, &never, |l| l.math("x", false)),
        MathSlot::Image { cols, rows } if cols > 0 && rows > 0
    ));
    // Failed: the text fallback.
    fx.st.cache.cache.get_mut(&fk).unwrap().failed = true;
    fx.st.cache.cache.get_mut(&mk).unwrap().failed = true;
    assert_eq!(
        fx.lookup(font, None, &never, |l| l.mermaid("a")),
        MermaidSlot::Text
    );
    assert_eq!(
        fx.lookup(font, None, &never, |l| l.math("x", false)),
        MathSlot::Raw
    );
}

#[test]
fn a_pictures_reserved_size_survives_a_re_raster() {
    // `layout_px` is the SVG's intrinsic size, set by the first result: filing a sharper raster
    // of the same picture must not change the cells it reserves.
    let mut fx = Fixture::new();
    let key = PathBuf::from(mermaid_fence_url("a"));
    fx.st.cache.cache.insert(
        key.clone(),
        MdImgEntry {
            layout_px: Some((400, 200)),
            decoded: Some(Arc::new(DynamicImage::new_rgba8(800, 400))),
            ..MdImgEntry::default()
        },
    );
    let before = fx.lookup(Some((8, 16)), None, &never, |l| l.mermaid("a"));
    fx.st.cache.cache.get_mut(&key).unwrap().decoded =
        Some(Arc::new(DynamicImage::new_rgba8(3200, 1600)));
    let after = fx.lookup(Some((8, 16)), None, &never, |l| l.mermaid("a"));
    assert_eq!(before, after);
}

#[test]
fn with_slots_hands_the_renderer_the_same_answers() {
    let fx = Fixture::new();
    let (img, fence, math, on) = fx.lookup(Some((8, 16)), None, &never, |l| {
        l.with_slots(true, |s| {
            (
                (s.image)("missing.png", None),
                (s.mermaid)("a"),
                (s.math)("x", true),
                (s.mermaid_on, s.math_on, s.remote_on),
            )
        })
    });
    assert_eq!(img, ImageSlot::Unavailable);
    assert_eq!(fence, MermaidSlot::Loading);
    assert_eq!(math, MathSlot::Loading);
    assert_eq!(on, (true, true, true));
}

#[test]
fn a_download_without_a_sandboxed_cache_root_does_not_panic() {
    // `test-hooks` is switched on for builds that are not tests (dev-dependency feature
    // unification), so a download with no test root must run like production, not assert.
    static CALLS: AtomicUsize = AtomicUsize::new(0);
    clear_test_cache_root();
    let (mut st, rx) = State::new().with_fetches();
    // The stand-in never touches the network or the disk.
    st.fetcher = |_, _| {
        CALLS.fetch_add(1, Ordering::SeqCst);
        false
    };
    let url = "https://example.com/no-root.png";
    assert!(!st.pictures().request_remote(url));
    let res = rx.recv().unwrap();
    assert_eq!((res.url.as_str(), res.ok), (url, false));
    assert_eq!(CALLS.load(Ordering::SeqCst), 1);
}

#[test]
fn a_cached_picture_is_reported_so_a_host_can_skip_building_the_plugin() {
    let mut st = State::new();
    let code = "graph TD; Z-->Y";
    let key = PathBuf::from(mermaid_fence_url(code));
    assert!(!st.pictures().is_cached(&key));
    st.pictures()
        .request_fence(code.into(), None, "fence:mermaid", 512);
    assert!(st.pictures().is_cached(&key));
}

#[test]
fn the_mermaid_rows_cap_takes_a_valid_setting_and_defaults_otherwise() {
    assert_eq!(mermaid_rows_cap(9), 9);
    assert_eq!(mermaid_rows_cap(i64::from(u16::MAX)), u16::MAX);
    for bad in [0, -1, i64::from(u16::MAX) + 1, i64::MIN] {
        assert_eq!(mermaid_rows_cap(bad), DEFAULT_MERMAID_ROWS, "{bad}");
    }
}

#[test]
fn a_fence_shrinks_to_leave_the_viewport_its_chrome_but_never_below_four_rows() {
    // A viewport not known yet keeps the cap; a known one leaves two rows.
    assert_eq!(mermaid_fit_rows(24, 0), 24);
    assert_eq!(mermaid_fit_rows(24, 40), 24);
    assert_eq!(mermaid_fit_rows(24, 20), 18);
    assert_eq!(mermaid_fit_rows(9, 40), 9);
    assert_eq!(mermaid_fit_rows(24, 6), 4);
    assert_eq!(mermaid_fit_rows(24, 1), 4);
    assert_eq!(mermaid_fit_rows(2, 40), 4);
}
