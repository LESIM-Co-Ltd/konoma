//! What a slide's draw teaches it: the raster size cap (`SlideRasterCap`), how the retries after a
//! refusal go (`decode_with_smaller_retries`), what is remembered from them
//! (`record_slide_attempts` / `decode_with_cap`), and that a request the user moved on from
//! teaches nothing.

use super::*;
use crate::app::office_doc::{DocPicture, LoadedDocument, PictureSource, SlideRasterCap};
use crate::app::{decode_with_cap, decode_with_smaller_retries, md_decode_bytes_why};
use crate::config::Config;
use crate::preview::image::ImageFailure;
use crate::preview::svg::RasterError;
use crate::preview::svg_guard::SvgFail;
use crate::test_support::unique_tmp;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

const SVG: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="30"><rect width="40" height="30" fill="red"/></svg>"#;

fn px_image() -> image::DynamicImage {
    image::DynamicImage::new_rgba8(2, 2)
}

fn fail(f: SvgFail) -> ImageFailure {
    ImageFailure::Svg(f)
}

/// Runs the whole slide decision with an `attempt` that fails with `f` at every size above
/// `works_at`; returns the sizes tried, the result and the cap afterwards at `now`.
fn run(
    start: u32,
    floor: u32,
    works_at: u32,
    f: ImageFailure,
    now: Instant,
) -> (Vec<u32>, Result<(), ImageFailure>, SlideRasterCap) {
    let cap = SlideRasterCap::default();
    let tried = RefCell::new(Vec::new());
    let res = decode_with_cap(Some(&cap), start, floor, &|| false, &|| now, |px| {
        tried.borrow_mut().push(px);
        if px <= works_at {
            Ok(px_image())
        } else {
            Err(f)
        }
    });
    (tried.into_inner(), res.map(|_| ()), cap)
}

// ---- the cap itself -------------------------------------------------------------------------

#[test]
fn a_cap_is_never_raised_and_never_below_one() {
    let cap = SlideRasterCap::default();
    let now = Instant::now();
    assert_eq!(cap.get(now), None);
    cap.lower_permanent(0);
    assert_eq!(cap.get(now), Some(1));
    let cap = SlideRasterCap::default();
    cap.lower_permanent(2000);
    cap.lower_permanent(3000);
    assert_eq!(cap.get(now), Some(2000));
    cap.lower_permanent(1500);
    assert_eq!(cap.get(now), Some(1500));
}

#[test]
fn a_cap_from_a_timeout_expires_but_a_permanent_one_does_not() {
    let t0 = Instant::now();
    let life = crate::app::office_doc::TIMEOUT_CAP_LIFETIME;
    let cap = SlideRasterCap::default();
    cap.lower_timed_out(1000, t0);
    assert_eq!(cap.get(t0), Some(1000));
    assert_eq!(cap.get(t0 + life - Duration::from_secs(1)), Some(1000));
    assert_eq!(cap.get(t0 + life), None, "expired exactly at its lifetime");
    assert_eq!(cap.get(t0 + life * 100), None);
    // The smaller of the two is in force, and the permanent one outlives the other.
    cap.lower_permanent(1500);
    assert_eq!(cap.get(t0), Some(1000));
    assert_eq!(cap.get(t0 + life), Some(1500));
    cap.lower_permanent(800);
    assert_eq!(cap.get(t0), Some(800));
    assert_eq!(cap.get(t0 + life * 100), Some(800));
}

#[test]
fn a_live_timed_out_cap_keeps_its_clock_and_an_expired_one_is_replaced() {
    let t0 = Instant::now();
    let life = crate::app::office_doc::TIMEOUT_CAP_LIFETIME;
    let cap = SlideRasterCap::default();
    cap.lower_timed_out(1000, t0);
    // A higher size later does not raise it nor restart its clock.
    cap.lower_timed_out(2000, t0 + life / 2);
    assert_eq!(cap.get(t0 + life / 2), Some(1000));
    assert_eq!(cap.get(t0 + life), None);
    // A lower size later replaces it and starts its own clock.
    cap.lower_timed_out(1000, t0);
    cap.lower_timed_out(500, t0 + life / 2);
    assert_eq!(cap.get(t0 + life / 2), Some(500));
    assert_eq!(cap.get(t0 + life), Some(500));
    assert_eq!(cap.get(t0 + life / 2 + life), None);
    // An expired one gives way to a higher size.
    let cap = SlideRasterCap::default();
    cap.lower_timed_out(500, t0);
    cap.lower_timed_out(900, t0 + life * 2);
    assert_eq!(cap.get(t0 + life * 2), Some(900));
    // Zero is one.
    cap.lower_timed_out(0, t0 + life * 2);
    assert_eq!(cap.get(t0 + life * 2), Some(1));
}

// ---- the sizes tried -------------------------------------------------------------------------

fn sizes(start: u32, floor: u32, f: ImageFailure, works_at: u32) -> Vec<u32> {
    let tried = RefCell::new(Vec::new());
    let _ = decode_with_smaller_retries(start, floor, true, &|| false, |px| {
        tried.borrow_mut().push(px);
        if px <= works_at {
            Ok(px_image())
        } else {
            Err(f)
        }
    });
    tried.into_inner()
}

#[test]
fn a_cost_or_memory_refusal_halves_down_to_the_floor() {
    for f in [SvgFail::TooHeavy, SvgFail::Memory] {
        assert_eq!(
            sizes(4096, 800, fail(f), 0),
            [4096, 2048, 1024, 800],
            "{f:?}"
        );
        assert_eq!(sizes(4096, 800, fail(f), 1024), [4096, 2048, 1024], "{f:?}");
    }
}

#[test]
fn a_timeout_is_retried_once_at_half_and_then_goes_straight_to_the_floor() {
    let t = fail(SvgFail::Timeout);
    // Fails at every size: the first, half, then the floor itself (no 1024 on the way).
    assert_eq!(sizes(4096, 800, t, 0), [4096, 2048, 800]);
    assert_eq!(sizes(4096, 1280, t, 0), [4096, 2048, 1280]);
    // Half works: that is the end.
    assert_eq!(sizes(4096, 800, t, 2048), [4096, 2048]);
    // Half is already under the floor: the floor.
    assert_eq!(sizes(1000, 800, t, 0), [1000, 800]);
    // No room below: one attempt.
    assert_eq!(sizes(800, 800, t, 0), [800]);
    // A longer ladder is not walked: from 4096 the worst case is three draws, not four.
    assert!(sizes(4096, 800, t, 0).len() <= 3);
}

#[test]
fn a_fast_refusal_before_a_timeout_does_not_use_up_the_timeout_retry() {
    let tried = RefCell::new(Vec::new());
    let _ = decode_with_smaller_retries(4096, 800, true, &|| false, |px| {
        tried.borrow_mut().push(px);
        match px {
            4096 => Err(fail(SvgFail::TooHeavy)),
            _ => Err(fail(SvgFail::Timeout)),
        }
    });
    // TooHeavy halves; the first timeout halves once more; the second goes to the floor.
    assert_eq!(tried.into_inner(), [4096, 2048, 1024, 800]);
}

// ---- what is remembered ----------------------------------------------------------------------

#[test]
fn a_smaller_retry_that_worked_lowers_the_cap_to_the_size_used() {
    let now = Instant::now();
    let (tried, res, cap) = run(4096, 800, 2048, fail(SvgFail::TooHeavy), now);
    assert_eq!((tried, res), (vec![4096, 2048], Ok(())));
    assert_eq!(cap.get(now), Some(2048));
    assert_eq!(cap.get(now + Duration::from_secs(3600)), Some(2048), "kept");
    // Two refusals on the way: the cap is where it finally worked.
    let (_, res, cap) = run(4096, 800, 1024, fail(SvgFail::Memory), now);
    assert_eq!(res, Ok(()));
    assert_eq!(cap.get(now), Some(1024));
}

#[test]
fn a_draw_that_worked_at_once_teaches_nothing() {
    let now = Instant::now();
    let (tried, res, cap) = run(4096, 800, u32::MAX, fail(SvgFail::TooHeavy), now);
    assert_eq!((tried, res), (vec![4096], Ok(())));
    assert_eq!(cap.get(now), None);
}

#[test]
fn failing_at_every_size_records_the_floor() {
    let now = Instant::now();
    for f in [SvgFail::TooHeavy, SvgFail::Memory] {
        let (tried, res, cap) = run(4096, 1280, 0, fail(f), now);
        assert_eq!(res, Err(fail(f)));
        assert_eq!(tried.last(), Some(&1280));
        assert_eq!(cap.get(now), Some(1280), "{f:?}");
        assert_eq!(
            cap.get(now + Duration::from_secs(3600)),
            Some(1280),
            "{f:?}"
        );
    }
    // A failure that is not a refusal for size is final at once (never retried smaller).
    let (tried, res, _) = run(4096, 1280, 0, fail(SvgFail::Crashed), now);
    assert_eq!((tried, res), (vec![4096], Err(fail(SvgFail::Crashed))));
    let (tried, res, _) = run(4096, 1280, 0, ImageFailure::Corrupt, now);
    assert_eq!((tried, res), (vec![4096], Err(ImageFailure::Corrupt)));
}

#[test]
fn a_crash_gives_a_cap_that_expires() {
    // Pinned to the old behaviour before: a crash under memory pressure kept the slide blurry for
    // the rest of the session.
    let t0 = Instant::now();
    let life = crate::app::office_doc::TIMEOUT_CAP_LIFETIME;
    let (_, res, cap) = run(4096, 1280, 0, fail(SvgFail::Crashed), t0);
    assert!(res.is_err());
    assert_eq!(cap.get(t0), Some(1280));
    assert_eq!(cap.get(t0 + life - Duration::from_secs(1)), Some(1280));
    assert_eq!(cap.get(t0 + life), None, "the pressure may be gone");
}

#[test]
fn a_failure_that_does_not_depend_on_the_size_holds_the_floor_for_a_while() {
    let t0 = Instant::now();
    let life = crate::app::office_doc::TIMEOUT_CAP_LIFETIME;
    for f in [
        fail(SvgFail::TooDeep),
        fail(SvgFail::TooLarge),
        fail(SvgFail::TooComplex),
        fail(SvgFail::Invalid),
        ImageFailure::TooLarge,
        ImageFailure::Corrupt,
        ImageFailure::UnsupportedFormat,
    ] {
        // Never retried smaller, and no size is learned from it: the cap is the floor (the size
        // already shown), so the same request is not made again at once, and only for a while.
        let (tried, res, cap) = run(4096, 1280, 0, f, t0);
        assert_eq!((tried, res), (vec![4096], Err(f)), "{f:?}");
        assert_eq!(cap.get(t0), Some(1280), "{f:?}");
        assert_eq!(cap.get(t0 + life), None, "{f:?}");
    }
    // A cancelled draw holds nothing.
    let (_, _, cap) = run(4096, 1280, 0, ImageFailure::Cancelled, t0);
    assert_eq!(cap.get(t0), None);
    // An earlier size-dependent refusal still counts when the next attempt fails otherwise.
    let far = t0 + life * 100;
    let cap = SlideRasterCap::default();
    let res = decode_with_cap(Some(&cap), 4096, 800, &|| false, &|| t0, |px| match px {
        4096 => Err(fail(SvgFail::TooHeavy)),
        _ => Err(ImageFailure::Corrupt),
    });
    assert_eq!(res.err(), Some(ImageFailure::Corrupt));
    assert_eq!(cap.get(far), Some(2048));
    assert_eq!(cap.get(t0), Some(800));
}

/// What every class of failure leaves behind when a draw at `start` fails at every size with a
/// floor of `floor` (the raster on screen for a sharpening redraw).
#[test]
fn every_failure_class_leaves_the_slide_unable_to_ask_for_the_same_size_at_once() {
    let t0 = Instant::now();
    let life = crate::app::office_doc::TIMEOUT_CAP_LIFETIME;
    // (failure, cap right after, cap long after)
    let cases: [(ImageFailure, Option<u32>, Option<u32>); 11] = [
        (fail(SvgFail::TooHeavy), Some(400), Some(400)),
        (fail(SvgFail::Memory), Some(400), Some(400)),
        (fail(SvgFail::Timeout), Some(400), None),
        (fail(SvgFail::Crashed), Some(400), None),
        (fail(SvgFail::TooDeep), Some(400), None),
        (fail(SvgFail::TooLarge), Some(400), None),
        (fail(SvgFail::TooComplex), Some(400), None),
        (fail(SvgFail::Invalid), Some(400), None),
        (ImageFailure::TooLarge, Some(400), None),
        (ImageFailure::Corrupt, Some(400), None),
        (ImageFailure::UnsupportedFormat, Some(400), None),
    ];
    for (f, soon, late) in cases {
        let (_, res, cap) = run(2000, 400, 0, f, t0);
        assert_eq!(res, Err(f), "{f:?}");
        assert_eq!(cap.get(t0), soon, "{f:?}");
        assert_eq!(cap.get(t0 + life * 100), late, "{f:?}");
    }
}

/// Through the real wish path: a sharpening redraw that failed for a reason that has nothing to do
/// with the size must not be started again by every following frame.
#[test]
fn a_failed_sharpening_redraw_is_not_asked_for_again_by_every_frame() {
    let (mut app, _dir, cap, rx) = app_with_slide(Default::default());
    let url = "office-img://deck/slide-1";
    let key = PathBuf::from(url);
    app.md_image_cache.insert(
        key.clone(),
        MdImgEntry {
            decoded: Some(Arc::new(image::DynamicImage::new_rgba8(400, 225))),
            ..MdImgEntry::default()
        },
    );
    // The redraw at 2000 fails for good (a panic caught in the drawing, say), as the worker
    // records it, with the raster on screen as the floor; `apply_md_image` then leaves the entry
    // alone (a failed re-raster keeps its pixels).
    let res = decode_with_cap(Some(&cap), 2000, 400, &|| false, &Instant::now, |_| {
        Err(ImageFailure::Corrupt)
    });
    assert_eq!(res.err(), Some(ImageFailure::Corrupt));
    for _ in 0..5 {
        app.note_slide_raster_wish(url, &key, 2000);
        assert!(
            !app.md_image_cache[&key].reraster_inflight,
            "a redraw was started again"
        );
    }
    assert!(
        rx.recv_timeout(Duration::from_millis(300)).is_err(),
        "a worker was started"
    );
    // The premise: without what the failure recorded, the same wish does start a redraw.
    let (mut app, _dir, _cap, rx) = app_with_slide(Default::default());
    app.md_image_cache.insert(
        key.clone(),
        MdImgEntry {
            decoded: Some(Arc::new(image::DynamicImage::new_rgba8(400, 225))),
            ..MdImgEntry::default()
        },
    );
    app.note_slide_raster_wish(url, &key, 2000);
    assert!(app.md_image_cache[&key].reraster_inflight);
    assert!(rx.recv_timeout(Duration::from_secs(60)).is_ok());
}

#[test]
fn a_refusal_confirmed_before_a_cancellation_is_kept() {
    let t0 = Instant::now();
    let far = t0 + Duration::from_secs(3600);
    // Too heavy at 4096, then the user moves on while 2048 is being drawn.
    let moved = Cell::new(false);
    let cap = SlideRasterCap::default();
    let res = decode_with_cap(Some(&cap), 4096, 800, &|| moved.get(), &|| t0, |px| {
        if px == 4096 {
            Err(fail(SvgFail::TooHeavy))
        } else {
            moved.set(true);
            Err(ImageFailure::Cancelled)
        }
    });
    assert_eq!(res.err(), Some(ImageFailure::Cancelled));
    assert_eq!(
        cap.get(far),
        Some(2048),
        "4096 was refused, 2048 taught nothing"
    );
    // The same when the last attempt came back with a real refusal after the user moved on.
    let moved = Cell::new(false);
    let cap = SlideRasterCap::default();
    let _ = decode_with_cap(Some(&cap), 4096, 800, &|| moved.get(), &|| t0, |px| {
        if px != 4096 {
            moved.set(true);
        }
        Err(fail(SvgFail::TooHeavy))
    });
    assert_eq!(cap.get(far), Some(2048));
}

#[test]
fn a_timeout_cap_counts_its_lifetime_from_the_refusal() {
    let t0 = Instant::now();
    let life = crate::app::office_doc::TIMEOUT_CAP_LIFETIME;
    // The drawing takes 15 s before the refusal is known; the clock is read when it is recorded.
    let elapsed = Cell::new(Duration::ZERO);
    let cap = SlideRasterCap::default();
    let _ = decode_with_cap(
        Some(&cap),
        4096,
        800,
        &|| false,
        &|| t0 + elapsed.get(),
        |_| {
            elapsed.set(Duration::from_secs(15));
            Err(fail(SvgFail::Timeout))
        },
    );
    let learned = t0 + Duration::from_secs(15);
    assert_eq!(cap.get(learned + life - Duration::from_secs(1)), Some(800));
    assert_eq!(cap.get(learned + life), None);
}

#[test]
fn a_floor_above_the_start_caps_at_the_start() {
    let now = Instant::now();
    let (_, res, cap) = run(1000, 5000, 0, fail(SvgFail::TooHeavy), now);
    assert!(res.is_err());
    assert_eq!(cap.get(now), Some(1000));
}

#[test]
fn a_cap_that_came_from_a_timeout_expires_and_a_deterministic_one_does_not() {
    let t0 = Instant::now();
    let life = crate::app::office_doc::TIMEOUT_CAP_LIFETIME;
    let after = t0 + life + Duration::from_secs(1);
    // Half worked after a timeout.
    let (_, res, cap) = run(4096, 800, 2048, fail(SvgFail::Timeout), t0);
    assert_eq!(res, Ok(()));
    assert_eq!(cap.get(t0), Some(2048));
    assert_eq!(
        cap.get(t0 + life / 2),
        Some(2048),
        "a revisit soon after: no new wait"
    );
    assert_eq!(cap.get(after), None, "the load may be gone");
    // Timed out at every size: the floor, also only for a while.
    let (_, res, cap) = run(4096, 800, 0, fail(SvgFail::Timeout), t0);
    assert!(res.is_err());
    assert_eq!(cap.get(t0), Some(800));
    assert_eq!(cap.get(after), None);
    // Too heavy: the same on every run.
    let (_, _, cap) = run(4096, 800, 2048, fail(SvgFail::TooHeavy), t0);
    assert_eq!(cap.get(after), Some(2048));
    // A cost refusal that is followed by a timeout: the first part is kept, the second expires.
    let cap = SlideRasterCap::default();
    let res = decode_with_cap(Some(&cap), 4096, 800, &|| false, &|| t0, |px| match px {
        4096 => Err(fail(SvgFail::TooHeavy)),
        2048 => Err(fail(SvgFail::Timeout)),
        _ => Ok(px_image()),
    });
    assert!(res.is_ok());
    assert_eq!(cap.get(t0), Some(1024));
    assert_eq!(
        cap.get(after),
        Some(2048),
        "only what the cost refusal proved is left"
    );
}

#[test]
fn a_picture_that_is_not_a_slide_is_tried_once_and_teaches_nothing() {
    let tried = Cell::new(0);
    let res = decode_with_cap(None, 4096, 800, &|| false, &|| Instant::now(), |_| {
        tried.set(tried.get() + 1);
        Err(fail(SvgFail::TooHeavy))
    });
    assert_eq!(res.err(), Some(fail(SvgFail::TooHeavy)));
    assert_eq!(tried.get(), 1);
}

// ---- a request the user moved on from --------------------------------------------------------

/// The real decode path with the user having moved on: the drawing is asked for and refused
/// because `cancelled` is already true. In the unit tests the drawing runs in-process, which does
/// not look at `cancelled`, so the drawing process is pointed at a program that does not exist: it
/// is never started, since the cancellation is seen first.
fn with_unstartable_child<T>(f: impl FnOnce() -> T) -> T {
    crate::preview::svg_proc::with_real_child(PathBuf::from("/nonexistent/konoma-svg-child"), f)
}

#[test]
fn a_cancellation_stays_a_cancellation_through_every_layer() {
    with_unstartable_child(|| {
        let always = || true;
        let dir = unique_tmp("konoma_slide_cap_cancel");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.svg");
        std::fs::write(&file, SVG).unwrap();
        // The three entry points of the supervised drawing.
        assert_eq!(
            crate::preview::svg::rasterize_embedded(SVG, 800, &always).err(),
            Some(RasterError::Cancelled)
        );
        assert_eq!(
            crate::preview::svg::rasterize_untrusted(SVG, &file, 800, &always).err(),
            Some(RasterError::Cancelled)
        );
        assert_eq!(
            crate::preview::svg::rasterize(&file, 800, &always).err(),
            Some(RasterError::Cancelled)
        );
        // ... and the two decoders that wrap them for a document's pictures.
        assert_eq!(
            md_decode_bytes_why(SVG, 800, &always).err(),
            Some(ImageFailure::Cancelled)
        );
        assert_eq!(
            md_decode_image_why(&file, 800, &always).err(),
            Some(ImageFailure::Cancelled)
        );
        // Not cancelled, the same program is a failure of the drawing process, not a cancellation.
        let never = || false;
        assert_eq!(
            md_decode_bytes_why(SVG, 800, &never).err(),
            Some(fail(SvgFail::Crashed))
        );
    });
}

#[test]
fn a_slide_the_user_moved_on_from_keeps_its_cap_whatever_the_drawing_said() {
    let now = Instant::now();
    // The real decode path, cancelled: the whole sequence of the worker around it.
    with_unstartable_child(|| {
        let moved = Cell::new(false);
        let cap = SlideRasterCap::default();
        let res = decode_with_cap(Some(&cap), 4096, 800, &|| moved.get(), &|| now, |px| {
            moved.set(true); // the user scrolls away while the slide is being drawn
            md_decode_bytes_why(SVG, px, &|| moved.get())
        });
        assert_eq!(res.err(), Some(ImageFailure::Cancelled));
        assert_eq!(
            cap.get(now),
            None,
            "a cancelled draw says nothing about the slide"
        );
    });
    // Moved on while a real refusal was coming back: also nothing learned.
    for f in [
        fail(SvgFail::Timeout),
        fail(SvgFail::TooHeavy),
        fail(SvgFail::Memory),
        fail(SvgFail::Crashed),
        ImageFailure::Corrupt,
    ] {
        let moved = Cell::new(false);
        let cap = SlideRasterCap::default();
        let res = decode_with_cap(Some(&cap), 4096, 800, &|| moved.get(), &|| now, |_| {
            moved.set(true);
            Err(f)
        });
        assert_eq!(res.err(), Some(f));
        assert_eq!(cap.get(now), None, "{f:?}");
    }
    // A cancelled attempt that nobody asked to stop (the document was closed): nothing either.
    let cap = SlideRasterCap::default();
    let res = decode_with_cap(Some(&cap), 4096, 800, &|| false, &|| now, |_| {
        Err(ImageFailure::Cancelled)
    });
    assert_eq!(res.err(), Some(ImageFailure::Cancelled));
    assert_eq!(cap.get(now), None);
    // A cap that was already known is left as it was.
    let cap = SlideRasterCap::default();
    cap.lower_permanent(2000);
    let _ = decode_with_cap(Some(&cap), 4096, 800, &|| true, &|| now, |_| {
        Err(fail(SvgFail::TooHeavy))
    });
    assert_eq!(cap.get(now), Some(2000));
}

// ---- through the worker ----------------------------------------------------------------------

fn app_with_slide(
    scene: crate::preview::office::slide_draw::SlideScene,
) -> (
    App,
    crate::test_support::TmpDir,
    Arc<SlideRasterCap>,
    std::sync::mpsc::Receiver<MdImageResult>,
) {
    let dir = unique_tmp("konoma_slide_cap_worker");
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = App::new(dir.to_path_buf(), Config::default()).unwrap();
    let cap = Arc::new(SlideRasterCap::default());
    let mut pictures = HashMap::new();
    pictures.insert(
        "office-img://deck/slide-1".to_string(),
        DocPicture {
            source: PictureSource::Slide {
                scene: Arc::new(scene),
                media: Arc::new(HashMap::new()),
                render_truncated: Arc::new(AtomicBool::new(false)),
                raster_cap: cap.clone(),
            },
            dims: Some((960, 540)),
            raster: false,
        },
    );
    app.document = Some(Box::new(LoadedDocument {
        markdown: String::new(),
        picture_markdown: String::new(),
        truncated: false,
        render_truncated: Arc::new(AtomicBool::new(false)),
        pictures,
        slides: Vec::new(),
    }));
    let (tx, rx) = std::sync::mpsc::channel();
    app.attach_md_image_loader(tx);
    (app, dir, cap, rx)
}

#[test]
fn the_worker_stores_the_writers_cap_for_the_slide_it_drew() {
    use crate::preview::office::slide_draw::{
        hardening_tests::shadowed_slide, render_svg_cancellable,
    };
    let scene = shadowed_slide(2);
    let expected = render_svg_cancellable(&scene, &|_| None, &|| false).max_raster_px();
    assert!(expected > 0 && expected < 4096, "premise: {expected}");
    let (mut app, _dir, cap, rx) = app_with_slide(scene);
    let url = "office-img://deck/slide-1";
    app.spawn_office_picture_decode(url, PathBuf::from(url), Some(4096));
    let res = rx
        .recv_timeout(Duration::from_secs(60))
        .expect("the worker answers");
    assert!(res.image.is_ok(), "{:?}", res.image.err());
    assert_eq!(cap.get(Instant::now()), Some(expected));
    assert_eq!(app.slide_raster_cap(url), Some(expected));
}

#[test]
fn the_worker_draws_no_bigger_than_a_cap_an_earlier_draw_left() {
    use image::GenericImageView;
    let (mut app, _dir, cap, rx) = app_with_slide(Default::default());
    cap.lower_permanent(900);
    let url = "office-img://deck/slide-1";
    app.spawn_office_picture_decode(url, PathBuf::from(url), Some(4096));
    let res = rx.recv_timeout(Duration::from_secs(60)).expect("answer");
    let img = res.image.expect("drawn");
    let (w, h) = img.dimensions();
    assert!(w.max(h) <= 900, "{w}x{h}");
    assert_eq!(cap.get(Instant::now()), Some(900));
}

#[test]
fn the_worker_remembers_nothing_for_a_request_the_user_moved_on_from() {
    let (mut app, _dir, cap, rx) = app_with_slide(Default::default());
    let url = "office-img://deck/slide-1";
    // The preview has moved on before the worker looks: the shared generation differs.
    app.media_gen_shared
        .store(app.media_gen + 1, std::sync::atomic::Ordering::Relaxed);
    app.spawn_office_picture_decode(url, PathBuf::from(url), Some(4096));
    // A cancelled first decode sends nothing; give the worker time to finish.
    let _ = rx.recv_timeout(Duration::from_millis(1500));
    assert_eq!(cap.get(Instant::now()), None);
}

// ---- pixel work that is pending but is not "loading" -----------------------------------------

#[test]
fn a_sharpening_redraw_in_flight_is_pending_pixel_work_though_the_picture_is_not_loading() {
    let dir = unique_tmp("konoma_slide_cap_pending");
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = App::new(dir.to_path_buf(), Config::default()).unwrap();
    assert!(!app.md_pixels_pending());
    let key = PathBuf::from("office-img://deck/slide-1");
    app.md_image_cache.insert(
        key.clone(),
        MdImgEntry {
            decoded: Some(Arc::new(image::DynamicImage::new_rgba8(4, 4))),
            ..MdImgEntry::default()
        },
    );
    assert!(!app.md_images_loading() && !app.md_pixels_pending(), "idle");
    app.md_image_cache.get_mut(&key).unwrap().reraster_inflight = true;
    assert!(!app.md_images_loading(), "its old pixels are on screen");
    assert!(app.md_pixels_pending(), "the new ones are owed");
    app.md_image_cache.get_mut(&key).unwrap().reraster_inflight = false;
    assert!(!app.md_pixels_pending());
    // Everything `md_images_loading` covers is covered.
    app.md_image_cache.get_mut(&key).unwrap().decoded = None;
    assert!(app.md_images_loading() && app.md_pixels_pending());
}
