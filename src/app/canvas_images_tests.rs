//! The worker side of canvas pictures: keys, hashes, what is decoded and what is skipped, limits;
//! and the landing of an answer in the cache.

use super::seen_for_tests::decoded_count;
use super::*;
use konoma_plugin::view::{CanvasImage, CanvasView};

fn rgba(w: u32, h: u32, c: u8) -> ImageData {
    ImageData::Rgba {
        width: w,
        height: h,
        pixels: Arc::new(vec![c; (w * h * 4) as usize]),
    }
}

fn canvas_of(images: Vec<CanvasImage>) -> CanvasView {
    let mut c = CanvasView::new(100, 100);
    c.images = images;
    c
}

fn im(id: u64, data: ImageData) -> CanvasImage {
    CanvasImage::new(id, CellRect::new(0, 0, 2, 2), data)
}

fn none(id: u64) -> CanvasImage {
    CanvasImage::unchanged(id, CellRect::new(0, 0, 2, 2))
}

fn nothing_held() -> CanvasHeld {
    CanvasHeld::default()
}

/// One `decode_canvas` with this host's policy: its supervised-child SVG drawer, the raster size
/// 800, and (when given) the job's cancellation and the believed `trusted` claim.
#[allow(clippy::too_many_arguments)]
fn run(
    plugin: &str,
    c: &CanvasView,
    held: &CanvasHeld,
    shown: &Path,
    scope: &konoma_plugin::ReadScope,
    limits: CanvasLimits,
    cancel: Option<&crate::preview::office::Cancel>,
    trust_own_svg: bool,
) -> CanvasPictures {
    let cancelled = || cancel.is_some_and(|c| c.is_cancelled());
    let policy = konoma_plugin::canvas_pictures::CanvasPolicy::new(
        scope,
        &super::media_plugin::SvgChild,
        800,
    )
    .limits(limits)
    .cancelled(&cancelled)
    .trust_own_svg(trust_own_svg);
    decode_canvas(plugin, c, held, shown, &policy)
}

fn dc(
    plugin: &str,
    c: &CanvasView,
    held: &CanvasHeld,
    shown: &Path,
    scope: &konoma_plugin::ReadScope,
) -> CanvasPictures {
    run(
        plugin,
        c,
        held,
        shown,
        scope,
        CanvasLimits::PRODUCTION,
        None,
        false,
    )
}

fn dcl(
    plugin: &str,
    c: &CanvasView,
    held: &CanvasHeld,
    shown: &Path,
    scope: &konoma_plugin::ReadScope,
    limits: CanvasLimits,
) -> CanvasPictures {
    run(plugin, c, held, shown, scope, limits, None, false)
}

fn decode(plugin: &str, c: &CanvasView, held: &CanvasHeld) -> CanvasPictures {
    dc(
        plugin,
        c,
        held,
        Path::new("/x/a.spr"),
        &konoma_plugin::ReadScope::unrestricted(),
    )
}

fn small(limits: impl FnOnce(&mut CanvasLimits)) -> CanvasLimits {
    let mut l = CanvasLimits::PRODUCTION;
    limits(&mut l);
    l
}

fn decode_within(c: &CanvasView, held: &CanvasHeld, l: CanvasLimits) -> CanvasPictures {
    dcl(
        "pw",
        c,
        held,
        Path::new("/x/a.spr"),
        &konoma_plugin::ReadScope::unrestricted(),
        l,
    )
}

/// What a cache would hold after `got` landed: its fresh keys with their decoded bytes.
fn held_after(held: &CanvasHeld, got: &CanvasPictures) -> CanvasHeld {
    let mut h = held.clone();
    for (k, d) in &got.fresh {
        h.keys.insert(k.clone(), decoded_bytes(d));
    }
    for p in &got.places {
        if let Some(k) = &p.key {
            h.by_id.insert(p.id, k.clone());
        }
    }
    h
}

fn keys(p: &CanvasPictures) -> Vec<Option<String>> {
    p.places
        .iter()
        .map(|p| p.key.as_deref().map(str::to_string))
        .collect()
}

fn new_app(name: &str) -> (App, crate::test_support::TmpDir) {
    let dir = crate::test_support::unique_tmp(name);
    std::fs::create_dir_all(&dir).unwrap();
    let app = App::new(dir.to_path_buf(), crate::config::Config::default()).unwrap();
    (app, dir)
}

#[test]
fn the_hash_follows_every_part_of_every_kind() {
    let bytes = |s: &str| -> Arc<[u8]> { Arc::from(s.as_bytes()) };
    let enc = |s: &str| ImageData::Encoded(bytes(s));
    let svg = |s: &str, dir: Option<&str>, trusted: bool| ImageData::Svg {
        data: bytes(s),
        base_dir: dir.map(PathBuf::from),
        trusted,
    };
    let px = |w: u32, h: u32, v: u8, len: usize| ImageData::Rgba {
        width: w,
        height: h,
        pixels: Arc::new(vec![v; len]),
    };
    let ms = Duration::from_millis;
    let fr = |list: Vec<(ImageData, Duration)>| ImageData::Frames(list);
    // Every entry must differ from every other.
    let all: Vec<(&str, ImageData)> = vec![
        ("enc ab", enc("ab")),
        ("enc ac", enc("ac")),
        ("enc abc", enc("abc")),
        ("svg ab", svg("ab", None, false)),
        ("svg ac", svg("ac", None, false)),
        ("svg ab trusted", svg("ab", None, true)),
        ("svg ab dir x", svg("ab", Some("/x"), false)),
        ("svg ab dir y", svg("ab", Some("/y"), false)),
        ("rgba 2x4 a", px(2, 4, 1, 32)),
        ("rgba 4x2 a", px(4, 2, 1, 32)),
        ("rgba 2x4 b", px(2, 4, 2, 32)),
        ("rgba 1x1 a", px(1, 1, 1, 4)),
        ("rgba 1x1 b", px(1, 1, 2, 4)),
        ("file ab", ImageData::File(PathBuf::from("ab"))),
        ("file ac", ImageData::File(PathBuf::from("ac"))),
        ("frames 1@10", fr(vec![(px(1, 1, 1, 4), ms(10))])),
        ("frames 1@20", fr(vec![(px(1, 1, 1, 4), ms(20))])),
        ("frames 2@10", fr(vec![(px(1, 1, 2, 4), ms(10))])),
        (
            "frames 1,1",
            fr(vec![(px(1, 1, 1, 4), ms(10)), (px(1, 1, 1, 4), ms(10))]),
        ),
        (
            "frames 1,2",
            fr(vec![(px(1, 1, 1, 4), ms(10)), (px(1, 1, 2, 4), ms(10))]),
        ),
        ("frames enc ab", fr(vec![(enc("ab"), ms(10))])),
        ("frames enc ac", fr(vec![(enc("ac"), ms(10))])),
        ("frames svg ab", fr(vec![(svg("ab", None, false), ms(10))])),
        ("frames none", fr(vec![])),
    ];
    for (i, (na, a)) in all.iter().enumerate() {
        assert_eq!(content_hash(a), content_hash(a), "{na}: stable");
        for (nb, b) in &all[i + 1..] {
            assert_ne!(content_hash(a), content_hash(b), "{na} vs {nb}");
        }
    }
    // The same bytes as different kinds are different pictures.
    assert_ne!(
        content_hash(&enc("GIF8")),
        content_hash(&svg("GIF8", None, false))
    );
    assert_ne!(
        content_hash(&enc("a")),
        content_hash(&ImageData::File(PathBuf::from("a")))
    );
    // A delay long enough to need more than 32 bits still counts as a whole.
    let long = |s: u64| fr(vec![(px(1, 1, 1, 4), Duration::from_secs(s))]);
    assert_ne!(content_hash(&long(1 << 33)), content_hash(&long(1 << 34)));
}

// ---- cancellation and files --------------------------------------------------------------------

#[test]
fn a_cancelled_job_decodes_nothing() {
    let cancel = crate::preview::office::Cancel::new(|| true);
    let c = canvas_of(vec![im(1, rgba(9, 9, 9))]);
    let got = run(
        "pcx",
        &c,
        &nothing_held(),
        Path::new("/x"),
        &konoma_plugin::ReadScope::unrestricted(),
        CanvasLimits::PRODUCTION,
        Some(&cancel),
        false,
    );
    // Cancelled while the content was hashed: nothing is decoded, nothing is named.
    assert!(got.fresh.is_empty());
    assert!(got.places.iter().all(|p| p.key.is_none()));
    assert!(got.sources.is_empty() && !got.more);
}

#[test]
fn a_cancel_between_frames_stops_an_animation() {
    let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let c2 = count.clone();
    // Cancelled once the content was hashed (six frames: six questions) and the job started: from
    // the ninth question on, so it is the frames loop that stops.
    let cancel = crate::preview::office::Cancel::new(move || {
        c2.fetch_add(1, std::sync::atomic::Ordering::SeqCst) >= 9
    });
    let frames = ImageData::Frames(
        (0..6)
            .map(|i| (rgba(2, 2, i + 1), Duration::from_millis(30)))
            .collect(),
    );
    let got = run(
        "pcf",
        &canvas_of(vec![im(1, frames)]),
        &nothing_held(),
        Path::new("/x"),
        &konoma_plugin::ReadScope::unrestricted(),
        CanvasLimits::PRODUCTION,
        Some(&cancel),
        false,
    );
    assert!(matches!(
        got.fresh[0].1,
        CanvasDecoded::Failed(crate::preview::image::ImageFailure::Cancelled)
    ));
}

// ---- landing ---------------------------------------------------------------------------------------

#[test]
fn a_cancelled_picture_leaves_no_entry_behind() {
    let (mut app, _dir) = new_app("cvi_cancelled");
    let key = PathBuf::from("canvas://p/1/00000000000000aa");
    app.canvas_pictures_landed(CanvasPictures {
        scope_fp: 0,
        places: vec![CanvasPlace {
            id: 1,
            rect: CellRect::new(0, 0, 1, 1),
            key: Some(Arc::from("canvas://p/1/00000000000000aa")),
        }],
        fresh: vec![(
            key.clone(),
            CanvasDecoded::Failed(crate::preview::image::ImageFailure::Cancelled),
        )],
        waiting: 0,
        refused: 0,
        more: false,
        sources: Vec::new(),
    });
    assert!(
        !app.md_images.cache.contains_key(&key),
        "asked for again next time"
    );
}

#[test]
fn a_cancelled_picture_is_neither_progress_nor_a_failure_on_landing() {
    // `apply_md_image` also forgets a cancelled result, so the entry alone cannot tell the two
    // guards apart: the landing must not count it as an added picture (that would ask for another
    // answer on the strength of nothing) and must not touch the held bytes.
    let (mut app, _dir) = new_app("cvi_cancelled_progress");
    let key = PathBuf::from("canvas://p/1/00000000000000ab");
    let more = app.canvas_pictures_landed(CanvasPictures {
        scope_fp: 0,
        places: vec![CanvasPlace {
            id: 1,
            rect: CellRect::new(0, 0, 1, 1),
            key: Some(Arc::from("canvas://p/1/00000000000000ab")),
        }],
        fresh: vec![(
            key.clone(),
            CanvasDecoded::Failed(crate::preview::image::ImageFailure::Cancelled),
        )],
        waiting: 3,
        refused: 0,
        more: true,
        sources: Vec::new(),
    });
    assert!(!more, "a cancelled decode is not progress");
    assert!(!app.md_images.cache.contains_key(&key));
    assert_eq!(app.canvas_held_bytes(), 0);
}

#[test]
fn landing_inserts_prunes_and_forgets() {
    let (mut app, _dir) = new_app("cvi_landing");
    let c1 = canvas_of(vec![im(1, rgba(4, 4, 1)), im(2, rgba(4, 4, 2))]);
    let p1 = decode("pl", &c1, &nothing_held());
    let keys1 = keys(&p1);
    app.canvas_pictures_landed(p1);
    assert_eq!(app.md_images.cache.len(), 2);
    let placed: Vec<_> = app
        .canvas_places
        .iter()
        .map(|p| p.key.as_deref().map(str::to_string))
        .collect();
    assert_eq!(placed, keys1);
    // An unrelated (Markdown / media-diff) entry is not touched by a canvas landing.
    app.md_images
        .cache
        .insert(PathBuf::from("media-diff://o/00/1"), Default::default());
    // The next answer keeps picture 1 and replaces picture 2's content. Nothing has been drawn,
    // so no entry is "drawn from": the replaced content goes.
    let c2 = canvas_of(vec![im(1, rgba(4, 4, 1)), im(2, rgba(4, 4, 3))]);
    let held = {
        let mut h = nothing_held();
        h.keys = app
            .md_images
            .cache
            .iter()
            .filter(|(k, _)| crate::preview::markdown::is_canvas_url(&k.to_string_lossy()))
            .map(|(k, e)| (k.clone(), e.pixel_bytes()))
            .collect();
        h
    };
    let p2 = decode("pl", &c2, &held);
    assert_eq!(p2.fresh.len(), 1, "only the changed picture");
    let keys2 = keys(&p2);
    app.canvas_pictures_landed(p2);
    assert_eq!(keys2[0], keys1[0]);
    assert_ne!(keys2[1], keys1[1]);
    let has = |k: &Option<String>| {
        app.md_images
            .cache
            .contains_key(Path::new(k.as_ref().unwrap()))
    };
    assert!(has(&keys1[0]));
    assert!(!has(&keys1[1]), "the replaced one is dropped");
    assert!(has(&keys2[1]));
    assert!(app
        .md_images
        .cache
        .contains_key(&PathBuf::from("media-diff://o/00/1")));
    // A canvas without pictures drops them all.
    app.canvas_pictures_landed(decode("pl", &canvas_of(vec![]), &nothing_held()));
    assert!(app
        .md_images
        .cache
        .keys()
        .all(|k| !crate::preview::markdown::is_canvas_url(&k.to_string_lossy())));
    assert!(app.canvas_places.is_empty());
    // And forgetting clears whatever is left.
    app.canvas_pictures_landed(decode("pl", &c1, &nothing_held()));
    app.forget_canvas_pictures();
    assert!(app.canvas_places.is_empty());
    assert!(app
        .md_images
        .cache
        .keys()
        .all(|k| !crate::preview::markdown::is_canvas_url(&k.to_string_lossy())));
}

#[test]
fn the_entry_an_id_is_drawn_from_stays_while_its_new_content_waits() {
    let (mut app, _dir) = new_app("cvi_front");
    let first = decode(
        "pfr",
        &canvas_of(vec![im(1, rgba(4, 4, 1))]),
        &nothing_held(),
    );
    let old = first.places[0].key.clone().unwrap();
    app.canvas_pictures_landed(first);
    // The frame drew it: it is the entry of id 1.
    app.canvas_front.insert(1, old.clone());
    let second = decode(
        "pfr",
        &canvas_of(vec![im(1, rgba(4, 4, 2))]),
        &nothing_held(),
    );
    let new = second.places[0].key.clone().unwrap();
    assert_ne!(old, new);
    app.canvas_pictures_landed(second);
    assert!(
        app.md_images.cache.contains_key(Path::new(&*old)),
        "still drawn from"
    );
    assert!(app.md_images.cache.contains_key(Path::new(&*new)));
    assert_eq!(app.md_images.cache.len(), 2);
    // The new content failed for good: the old picture ends (the cells show). The frame only
    // moved the pointer: the entry itself goes when the next answer lands (the prune), never in a
    // frame.
    app.md_images
        .cache
        .get_mut(Path::new(&*new))
        .unwrap()
        .failed = true;
    let shown = app.canvas_shown_key(1, &new, 2, 2, 2);
    assert_eq!(&*shown, &*new);
    assert!(!app.canvas_front.contains_key(&1));
    assert!(
        app.md_images.cache.contains_key(Path::new(&*old)),
        "a frame removes no entry"
    );
    let next = decode(
        "pfr",
        &canvas_of(vec![im(1, rgba(4, 4, 2))]),
        &app.canvas_known_keys_for_test(),
    );
    app.canvas_pictures_landed(next);
    assert!(!app.md_images.cache.contains_key(Path::new(&*old)));
    assert!(app.md_images.cache.contains_key(Path::new(&*new)));
}

#[test]
fn while_the_new_content_has_nothing_to_draw_the_old_entry_is_shown() {
    let (mut app, _dir) = new_app("cvi_front2");
    let first = decode(
        "pfs",
        &canvas_of(vec![im(1, rgba(4, 4, 1))]),
        &nothing_held(),
    );
    let old = first.places[0].key.clone().unwrap();
    app.canvas_pictures_landed(first);
    app.canvas_front.insert(1, old.clone());
    let second = decode(
        "pfs",
        &canvas_of(vec![im(1, rgba(4, 4, 2))]),
        &nothing_held(),
    );
    let new = second.places[0].key.clone().unwrap();
    app.canvas_pictures_landed(second);
    // Neither has a protocol in this test; the new one is not failed: the old one is shown.
    assert_eq!(&*app.canvas_shown_key(1, &new, 2, 2, 2), &*old);
    // An id never drawn has nothing to fall back on.
    assert_eq!(&*app.canvas_shown_key(5, &new, 2, 2, 2), &*new);
    // The old entry going (evicted) leaves the new one as the only candidate.
    app.md_images.cache.remove(Path::new(&*old));
    assert_eq!(&*app.canvas_shown_key(1, &new, 2, 2, 2), &*new);
}

#[test]
fn a_picture_refused_for_the_memory_bound_has_no_entry_and_is_not_remembered_as_failed() {
    let (mut app, _dir) = new_app("cvi_refused");
    let c = canvas_of(vec![im(1, rgba(100, 100, 1)), im(2, rgba(100, 100, 2))]);
    let got = decode("pr", &c, &nothing_held());
    let (k1, k2) = (
        got.places[0].key.clone().unwrap(),
        got.places[1].key.clone().unwrap(),
    );
    // The app's bound is smaller than what the worker thought was free (a race): 60,000 bytes.
    let more = app.canvas_pictures_landed_within(got, 60_000);
    assert!(!more);
    assert!(app.md_images.cache.contains_key(Path::new(&*k1)));
    assert!(
        !app.md_images.cache.contains_key(Path::new(&*k2)),
        "refused: no entry, so not 'known' and not failed"
    );
    assert_eq!(app.canvas_held_bytes(), 40_000);
    // The worker is told what the app holds; the refused picture is decoded again.
    let held = app.canvas_known_keys_for_test();
    let again = decode("pr", &c, &held);
    assert_eq!(again.fresh.len(), 1);
    assert_eq!(
        again.fresh[0].0.to_string_lossy(),
        *k2,
        "the refused picture comes back"
    );
}

#[test]
fn what_the_canvas_no_longer_names_is_freed_before_the_memory_bound_is_judged() {
    let (mut app, _dir) = new_app("cvi_freed");
    // Picture 1 fills most of a 60,000-byte bound.
    app.canvas_pictures_landed_within(
        decode(
            "pz",
            &canvas_of(vec![im(1, rgba(100, 100, 1))]),
            &nothing_held(),
        ),
        60_000,
    );
    assert_eq!(app.canvas_held_bytes(), 40_000);
    // The next canvas has another picture only: the old one goes first, so this one fits.
    let next = decode(
        "pz",
        &canvas_of(vec![im(2, rgba(100, 100, 2))]),
        &nothing_held(),
    );
    let k2 = next.places[0].key.clone().unwrap();
    app.canvas_pictures_landed_within(next, 60_000);
    assert!(app.md_images.cache.contains_key(Path::new(&*k2)));
    assert_eq!(app.md_images.cache.len(), 1);
    assert_eq!(app.canvas_held_bytes(), 40_000);
}

#[test]
fn landing_says_when_to_ask_again_only_if_it_made_progress() {
    let (mut app, _dir) = new_app("cvi_more");
    let n = MAX_DECODES_PER_ANSWER as u64 + 4;
    let c = canvas_of(
        (0..n)
            .map(|i| im(i, rgba(1, 1, (i % 250) as u8 + 1)))
            .collect(),
    );
    let got = decode("pm", &c, &nothing_held());
    assert!(got.more);
    assert!(
        app.canvas_pictures_landed(got),
        "added pictures and waiting ones: ask again"
    );
    // Nothing added (everything refused): do not ask, or it would loop.
    let (mut app2, _d2) = new_app("cvi_more2");
    let got = decode("pm", &c, &nothing_held());
    assert!(!app2.canvas_pictures_landed_within(got, 0));
}

#[test]
fn no_known_keys_without_an_image_backend() {
    let (app, _dir) = new_app("cvi_nobackend");
    assert!(app.canvas_known_keys().is_none());
    assert!(!app.canvas_pictures_drawable());
}

#[test]
fn canvas_entries_are_never_evicted_by_the_cache_budget() {
    // Their pixels cannot be rebuilt (the bytes were the plugin's), so the budget must not drop them.
    let (mut app, _dir) = new_app("cvi_noevict");
    app.canvas_pictures_landed(decode(
        "pe",
        &canvas_of(vec![im(1, rgba(64, 64, 9))]),
        &nothing_held(),
    ));
    let key = PathBuf::from(app.canvas_places[0].key.as_deref().unwrap());
    app.begin_md_image_frame();
    app.begin_md_image_frame();
    app.evict_md_images_to(0);
    assert!(
        app.md_images.cache[&key].decoded.is_some(),
        "the pixels are still there"
    );
}

#[test]
fn canvas_urls_are_synthetic_and_distinct_from_every_other_scheme() {
    use crate::preview::markdown::{
        canvas_url, is_canvas_url, is_media_diff_url, is_synthetic_md_url,
    };
    let u = canvas_url("plug", 42, 0xab);
    assert_eq!(u, "canvas://plug/42/00000000000000ab");
    assert!(is_canvas_url(&u));
    assert!(is_synthetic_md_url(&u));
    assert!(!is_media_diff_url(&u));
    assert!(!is_canvas_url("media-diff://o/00/1"));
    assert!(!is_canvas_url("math://i1"));
    assert!(!is_canvas_url("/home/u/canvas://x"));
    assert_ne!(canvas_url("a", 1, 2), canvas_url("b", 1, 2));
    assert_ne!(canvas_url("a", 1, 2), canvas_url("a", 2, 2));
    assert_ne!(canvas_url("a", 1, 2), canvas_url("a", 1, 3));
}

// ---- the terminal's copy: kitty ids ------------------------------------------------------------

#[test]
fn kitty_ids_are_filed_under_the_picture_not_its_content() {
    let a = kitty_key(Path::new("canvas://p/1/00000000000000aa"));
    let b = kitty_key(Path::new("canvas://p/1/00000000000000bb"));
    assert_eq!(a, b, "new content takes over the ids of the old");
    assert_eq!(a, PathBuf::from("canvas://p/1"));
    assert_ne!(a, kitty_key(Path::new("canvas://p/2/00000000000000aa")));
    assert_ne!(a, kitty_key(Path::new("canvas://q/1/00000000000000aa")));
    // Everything else is filed under itself.
    for other in [
        "/home/u/a.png",
        "math://x",
        "media-diff://o/00/1",
        "canvas://p",
    ] {
        assert_eq!(kitty_key(Path::new(other)), PathBuf::from(other), "{other}");
    }
}

#[test]
fn the_ids_of_a_picture_that_is_gone_are_deleted_and_those_of_a_replaced_one_are_kept() {
    let (mut app, _dir) = new_app("cvi_sweep");
    app.md_images.use_kitty = true;
    let c = canvas_of(vec![im(1, rgba(4, 4, 1)), im(2, rgba(4, 4, 2))]);
    app.canvas_pictures_landed(decode("pk", &c, &nothing_held()));
    // The encodes pinned ids for both slots (and Markdown has its own, which stay).
    let slot1 = PathBuf::from("canvas://pk/1");
    let slot2 = PathBuf::from("canvas://pk/2");
    app.md_images
        .kitty_ids
        .insert(slot1.clone(), [vec![11, 12], vec![13], vec![]]);
    app.md_images
        .kitty_ids
        .insert(slot2.clone(), [vec![21], vec![], vec![]]);
    app.md_images
        .kitty_ids
        .insert(PathBuf::from("/doc/a.png"), [vec![99], vec![], vec![]]);
    // Picture 1's content changes and picture 2 leaves.
    let c2 = canvas_of(vec![im(1, rgba(4, 4, 7))]);
    app.canvas_pictures_landed(decode("pk", &c2, &nothing_held()));
    assert!(
        app.md_images.kitty_ids.contains_key(&slot1),
        "same picture: ids kept"
    );
    assert!(!app.md_images.kitty_ids.contains_key(&slot2));
    assert!(app
        .md_images
        .kitty_ids
        .contains_key(&PathBuf::from("/doc/a.png")));
    let escapes = app.take_kitty_deletes();
    assert_eq!(
        escapes,
        vec![crate::preview::kitty::build_delete(21, false)],
        "only the gone picture's image is deleted"
    );
    assert!(app.take_kitty_deletes().is_empty(), "once");
    // The show goes: everything the canvas pinned is deleted, Markdown's is not.
    app.forget_canvas_pictures();
    let mut gone: Vec<String> = app.take_kitty_deletes();
    gone.sort();
    let mut want: Vec<String> = [11, 12, 13]
        .iter()
        .map(|id| crate::preview::kitty::build_delete(*id, false))
        .collect();
    want.sort();
    assert_eq!(gone, want);
    assert!(app
        .md_images
        .kitty_ids
        .contains_key(&PathBuf::from("/doc/a.png")));
}

#[test]
fn deletes_go_only_to_a_kitty_terminal_and_follow_the_tmux_wrapping() {
    let (mut app, _dir) = new_app("cvi_deletes");
    app.md_images.kitty_deletes = vec![5];
    app.md_images.use_kitty = false;
    assert!(
        app.take_kitty_deletes().is_empty(),
        "no kitty, nothing is written"
    );
    assert!(
        app.md_images.kitty_deletes.is_empty(),
        "and the queue does not pile up"
    );
    app.md_images.use_kitty = true;
    app.md_images.kitty_deletes = vec![5, 6];
    app.md_images.kitty_is_tmux = true;
    assert_eq!(
        app.take_kitty_deletes(),
        vec![
            crate::preview::kitty::build_delete(5, true),
            crate::preview::kitty::build_delete(6, true)
        ]
    );
}

// ---- batch F: the host keeps what it deferred or refused ---------------------------------------------

/// Land `got` the way the app does and return what the next job would be told.
fn land(app: &mut App, got: CanvasPictures) -> CanvasHeld {
    app.canvas_pictures_landed(got);
    app.canvas_known_keys_for_test()
}

#[test]
fn a_picture_deferred_by_the_work_limits_is_decoded_when_a_later_answer_says_unchanged() {
    let (mut app, _dir) = new_app("cvi_kept");
    let n = MAX_DECODES_PER_ANSWER as u64 + 8;
    let sends = canvas_of(
        (0..n)
            .map(|i| im(i, rgba(1, 1, (i % 250) as u8 + 1)))
            .collect(),
    );
    let held = land(&mut app, decode("pk", &sends, &nothing_held()));
    assert_eq!(app.md_images.cache.len(), MAX_DECODES_PER_ANSWER);
    assert_eq!(held.kept.len(), 8, "the host kept the 8 it deferred");
    // The plugin does what the API tells it: pixels once, then `unchanged` for all.
    let unchanged = canvas_of((0..n).map(none).collect());
    let mut held = held;
    for _ in 0..3 {
        held = land(&mut app, decode("pk", &unchanged, &held));
    }
    assert_eq!(app.md_images.cache.len(), n as usize, "all arrived");
    assert!(app.canvas_kept.is_empty(), "nothing is kept any more");
    assert!(app.canvas_places.iter().all(|p| p.key.is_some()));
}

#[test]
fn a_picture_refused_for_the_memory_bound_is_decoded_later_even_when_the_plugin_says_unchanged() {
    let (mut app, _dir) = new_app("cvi_kept_mem");
    let (a, b) = (rgba(100, 100, 231), rgba(100, 100, 232));
    let hb = content_hash(&b);
    let l = small(|l| l.held_bytes = 60_000);
    let both = canvas_of(vec![im(1, a), im(2, b)]);
    let got = decode_within(&both, &nothing_held(), l);
    assert_eq!((got.fresh.len(), got.refused), (1, 1));
    app.canvas_pictures_landed_within(got, 60_000);
    assert_eq!(app.canvas_kept.len(), 1);
    assert_eq!(app.canvas_kept[&2].needs, Some(40_000));
    assert_eq!(decoded_count(hb), 1);
    // Unchanged for both: still no room, and the picture is NOT decoded again to be refused.
    for _ in 0..3 {
        let held = app.canvas_known_keys_for_test();
        let again = decode_within(&canvas_of(vec![none(1), none(2)]), &held, l);
        assert!(again.fresh.is_empty());
        assert_eq!(again.refused, 1);
        app.canvas_pictures_landed_within(again, 60_000);
    }
    assert_eq!(
        decoded_count(hb),
        1,
        "not decoded again while there is no room"
    );
    // Picture 1 goes: there is room, and picture 2 is decoded without being sent again.
    let held = app.canvas_known_keys_for_test();
    let later = decode_within(&canvas_of(vec![none(2)]), &held, l);
    assert_eq!(later.fresh.len(), 1);
    assert_eq!(decoded_count(hb), 2);
    app.canvas_pictures_landed_within(later, 60_000);
    assert!(app.canvas_kept.is_empty());
    assert_eq!(app.md_images.cache.len(), 1);
}

#[test]
fn a_refusal_at_landing_keeps_the_source_with_what_it_needs() {
    let (mut app, _dir) = new_app("cvi_kept_land");
    let c = canvas_of(vec![im(1, rgba(100, 100, 233)), im(2, rgba(100, 100, 234))]);
    // The worker thought there was room (a race): the landing refuses the second.
    let got = decode("pkl", &c, &nothing_held());
    let k2 = got.places[1].key.clone().unwrap();
    app.canvas_pictures_landed_within(got, 60_000);
    assert_eq!(app.canvas_kept.len(), 1);
    assert_eq!(app.canvas_kept[&2].key.as_deref(), Some(&*k2));
    assert_eq!(app.canvas_kept[&2].needs, Some(40_000));
    // With room (the bound raised in the test), the next answer, unchanged, brings it in.
    let held = app.canvas_known_keys_for_test();
    let next = decode("pkl", &canvas_of(vec![none(1), none(2)]), &held);
    assert_eq!(next.fresh.len(), 1);
    app.canvas_pictures_landed(next);
    assert!(app.md_images.cache.contains_key(Path::new(&*k2)));
    assert!(app.canvas_kept.is_empty());
}

#[test]
fn newer_content_for_a_kept_id_replaces_the_kept_data() {
    let (mut app, _dir) = new_app("cvi_kept_new");
    let (old, new) = (rgba(7, 3, 235), rgba(7, 3, 236));
    let (hold, hnew) = (content_hash(&old), content_hash(&new));
    let l = small(|l| l.decodes = 1);
    let first = canvas_of(vec![im(1, rgba(7, 3, 237)), im(2, old)]);
    let got = decode_within(&first, &nothing_held(), l);
    assert_eq!(got.waiting, 1);
    app.canvas_pictures_landed(got);
    assert_eq!(app.canvas_kept.len(), 1);
    // Id 2 now has other content: the kept data is replaced, the old content is never decoded.
    let held = app.canvas_known_keys_for_test();
    let got = decode_within(&canvas_of(vec![none(1), im(2, new)]), &held, l);
    app.canvas_pictures_landed(got);
    assert_eq!(
        decoded_count(hold),
        0,
        "the replaced content was never decoded"
    );
    assert_eq!(decoded_count(hnew), 1);
    assert!(app.canvas_kept.is_empty());
    assert_eq!(app.md_images.cache.len(), 2);
}

#[test]
fn what_is_kept_goes_when_the_id_leaves_the_canvas_or_the_show_is_dropped() {
    let (mut app, _dir) = new_app("cvi_kept_gone");
    let l = small(|l| l.decodes = 1);
    let two = canvas_of(vec![im(1, rgba(1, 1, 238)), im(2, rgba(1, 1, 239))]);
    app.canvas_pictures_landed(decode_within(&two, &nothing_held(), l));
    assert!(app.canvas_kept.contains_key(&2));
    // The next canvas does not name id 2.
    let held = app.canvas_known_keys_for_test();
    app.canvas_pictures_landed(decode_within(&canvas_of(vec![none(1)]), &held, l));
    assert!(app.canvas_kept.is_empty(), "released with the id");
    // And with the show.
    app.canvas_pictures_landed(decode_within(&two, &nothing_held(), l));
    app.forget_canvas_pictures();
    assert!(app.canvas_kept.is_empty());
}

#[test]
fn a_front_entry_survives_until_an_answer_lands_so_a_flip_flop_never_shows_the_old_content() {
    // The picture flips between A and B. A job holds a snapshot with both; a frame moves the
    // pointer to B meanwhile; the answer names A. A must still be there.
    let (mut app, _dir) = new_app("cvi_flip");
    let a = canvas_of(vec![im(1, rgba(4, 4, 240))]);
    let b = canvas_of(vec![im(1, rgba(4, 4, 241))]);
    let ga = decode("pfl", &a, &nothing_held());
    let ka = ga.places[0].key.clone().unwrap();
    app.canvas_pictures_landed(ga);
    app.canvas_front.insert(1, ka.clone());
    let held = app.canvas_known_keys_for_test();
    let gb = decode("pfl", &b, &held);
    let kb = gb.places[0].key.clone().unwrap();
    app.canvas_pictures_landed(gb);
    let snapshot = app.canvas_known_keys_for_test();
    assert!(snapshot.keys.contains_key(Path::new(&*ka)));
    // The frame draws B (failed-path stand-in: the pointer moves, nothing is removed).
    app.md_images.cache.get_mut(Path::new(&*kb)).unwrap().failed = true;
    app.canvas_shown_key(1, &kb, 2, 2, 2);
    assert!(
        app.md_images.cache.contains_key(Path::new(&*ka)),
        "no frame drops an entry"
    );
    // The answer of the job names A again.
    let ga2 = decode("pfl", &a, &snapshot);
    assert!(ga2.fresh.is_empty(), "A is held, so nothing is decoded");
    app.canvas_pictures_landed(ga2);
    assert!(
        app.md_images.cache.contains_key(Path::new(&*ka)),
        "the placement names an entry that is there"
    );
}

#[test]
fn the_input_budget_is_applied_before_hashing_and_the_unhashed_picture_keeps_its_data() {
    let (mut app, _dir) = new_app("cvi_prehash");
    // 40,000 bytes each; the budget 70,000: the third starts after 80,000 were read.
    let l = small(|l| l.input_bytes = 70_000);
    let c = canvas_of(vec![
        im(1, rgba(100, 100, 21)),
        im(2, rgba(100, 100, 22)),
        im(3, rgba(100, 100, 23)),
    ]);
    let got = decode_within(&c, &nothing_held(), l);
    assert_eq!(got.fresh.len(), 2);
    assert_eq!(got.waiting, 1);
    assert!(got.places[2].key.is_none(), "not hashed: no key yet");
    let unhashed: Vec<_> = got
        .sources
        .iter()
        .filter(|(_, k)| k.key.is_none())
        .collect();
    assert_eq!(unhashed.len(), 1);
    assert_eq!(unhashed[0].0, 3);
    assert!(unhashed[0].1.hash.is_none());
    assert!(got.more);
    app.canvas_pictures_landed(got);
    assert!(app.canvas_kept.contains_key(&3));
    // Unchanged: the kept one goes first, is hashed and decoded.
    let held = app.canvas_known_keys_for_test();
    let next = decode_within(&canvas_of(vec![none(1), none(2), none(3)]), &held, l);
    assert_eq!(next.fresh.len(), 1, "only the third");
    assert!(next.places[2].key.is_some());
    app.canvas_pictures_landed(next);
    assert_eq!(app.md_images.cache.len(), 3);
    assert!(app.canvas_kept.is_empty());
}

// ---- batch F: the bytes counted ----------------------------------------------------------------------

fn png_of(img: image::DynamicImage) -> ImageData {
    let mut out = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
        .unwrap();
    ImageData::Encoded(Arc::from(out))
}

#[test]
fn the_memory_bound_counts_the_pixel_format_the_cache_keeps() {
    let (mut app, _dir) = new_app("cvi_bpp");
    let rgb8 = png_of(image::DynamicImage::ImageRgb8(image::RgbImage::new(
        100, 100,
    )));
    let rgba16 = png_of(image::DynamicImage::ImageRgba16(image::ImageBuffer::new(
        100, 100,
    )));
    let c = canvas_of(vec![im(1, rgb8), im(2, rgba16)]);
    let got = decode("pbp", &c, &nothing_held());
    assert_eq!(decoded_bytes(&got.fresh[0].1), 30_000, "3 bytes a pixel");
    assert_eq!(decoded_bytes(&got.fresh[1].1), 80_000, "8 bytes a pixel");
    app.canvas_pictures_landed(got);
    // The worker, the landing and the cache count the same.
    assert_eq!(app.canvas_held_bytes(), 110_000);
    // A bound of 35,000 takes the RGB8 picture (30,000), which a 4-bytes count (40,000) refused.
    let tight = decode_within(
        &canvas_of(vec![im(
            1,
            png_of(image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
                100,
                100,
                image::Rgb([9, 9, 9]),
            ))),
        )]),
        &nothing_held(),
        small(|l| l.held_bytes = 35_000),
    );
    assert_eq!(tight.fresh.len(), 1);
    assert!(!matches!(tight.fresh[0].1, CanvasDecoded::Failed(_)));
}

// ---- round 3: a new content replaces the old one of its id; the asks; the kept bound -----------------

/// The key of every placement, unwrapped.
fn key_list(p: &CanvasPictures) -> Vec<Arc<str>> {
    p.places.iter().map(|p| p.key.clone().unwrap()).collect()
}

/// Mark the placements as drawn (what the frame does once a picture has a protocol): the id is
/// drawn from its entry. After the landing that put the entries in the cache.
fn draw_places(app: &mut App, places: &[CanvasPlace]) {
    for pl in places {
        app.canvas_front.insert(pl.id, pl.key.clone().unwrap());
    }
}

#[test]
fn a_page_flip_replaces_the_pictures_of_its_ids_even_with_the_bound_full() {
    // Four ids of 40,000 bytes fill a 160,000-byte bound. The next page puts new content under the
    // same ids: each replaces its id's old picture (which would otherwise hold the room for ever,
    // the new content being refused and the old page staying on screen).
    let (mut app, _dir) = new_app("cvi_flip");
    let bound = 160_000;
    let l = small(|l| l.held_bytes = bound);
    let page = |c: u8| canvas_of((0..4).map(|i| im(i, rgba(100, 100, c + i as u8))).collect());
    let first = decode_within(&page(60), &nothing_held(), l);
    assert_eq!(first.fresh.len(), 4);
    let old = key_list(&first);
    let drawn = first.places.clone();
    app.canvas_pictures_landed_within(first, bound);
    draw_places(&mut app, &drawn);
    assert_eq!(app.canvas_held_bytes(), bound);
    let second = decode_within(&page(70), &app.canvas_known_keys_for_test(), l);
    assert_eq!(second.refused, 0, "the old entries are being replaced");
    assert_eq!(second.fresh.len(), 4, "all four are decoded at once");
    let new = key_list(&second);
    let more = app.canvas_pictures_landed_within(second, bound);
    assert!(!more, "nothing waits");
    for k in &new {
        assert!(app.md_images.cache.contains_key(Path::new(&**k)), "{k}");
    }
    for k in &old {
        assert!(!app.md_images.cache.contains_key(Path::new(&**k)), "{k}");
    }
    assert_eq!(app.canvas_held_bytes(), bound);
    assert!(app.canvas_kept.is_empty());
}

#[test]
fn the_front_of_another_id_is_not_replaced_by_a_new_content() {
    // Id 1 and id 2 hold 80,000 bytes each in a 160,000-byte bound; id 1 gets new content of
    // 80,000. Id 2 (unchanged) stays held, and id 1's old picture is what makes room.
    let (mut app, _dir) = new_app("cvi_flip_other");
    let bound = 160_000;
    let l = small(|l| l.held_bytes = bound);
    let big = |c| rgba(141, 141, c); // 79,524 bytes
    let first = decode_within(
        &canvas_of(vec![im(1, big(61)), im(2, big(62))]),
        &nothing_held(),
        l,
    );
    let keys1 = key_list(&first);
    let drawn = first.places.clone();
    app.canvas_pictures_landed_within(first, bound);
    draw_places(&mut app, &drawn);
    let second = decode_within(
        &canvas_of(vec![im(1, big(63)), none(2)]),
        &app.canvas_known_keys_for_test(),
        l,
    );
    assert_eq!(second.fresh.len(), 1);
    let k_new = second.places[0].key.clone().unwrap();
    app.canvas_pictures_landed_within(second, bound);
    assert!(app.md_images.cache.contains_key(Path::new(&*k_new)));
    assert!(app.md_images.cache.contains_key(Path::new(&*keys1[1])));
    assert!(!app.md_images.cache.contains_key(Path::new(&*keys1[0])));
}

#[test]
fn a_new_content_refused_for_memory_ends_the_old_picture_of_its_id_and_comes_when_room_grows() {
    let (mut app, _dir) = new_app("cvi_refuse_front");
    let bound = 160_000;
    let l = small(|l| l.held_bytes = bound);
    let first = decode_within(
        &canvas_of(
            (0..4)
                .map(|i| im(i, rgba(100, 100, 80 + i as u8)))
                .collect(),
        ),
        &nothing_held(),
        l,
    );
    let old = key_list(&first);
    let drawn = first.places.clone();
    app.canvas_pictures_landed_within(first, bound);
    draw_places(&mut app, &drawn);
    // Id 0 now has content of 79,524 bytes; the other three keep 120,000: it cannot fit.
    let second = decode_within(
        &canvas_of(vec![im(0, rgba(141, 141, 90)), none(1), none(2), none(3)]),
        &app.canvas_known_keys_for_test(),
        l,
    );
    assert_eq!((second.fresh.len(), second.refused), (0, 1));
    let more = app.canvas_pictures_landed_within(second, bound);
    assert!(!more, "no room for it yet: asking would not help");
    assert!(
        !app.canvas_front.contains_key(&0),
        "the old picture of id 0 is not shown in place of the new one"
    );
    assert!(
        !app.md_images.cache.contains_key(Path::new(&*old[0])),
        "and its pixels are freed"
    );
    assert_eq!(app.canvas_kept[&0].needs, Some(79_524));
    assert_eq!(app.canvas_held_bytes(), 120_000);
    // Id 3 leaves the canvas: the next answer has the room and brings the picture in, from the
    // source the host kept, though the plugin only says `unchanged`.
    let third = decode_within(
        &canvas_of(vec![none(0), none(1), none(2)]),
        &app.canvas_known_keys_for_test(),
        l,
    );
    assert_eq!(third.fresh.len(), 1);
    app.canvas_pictures_landed_within(third, bound);
    assert!(app.canvas_kept.is_empty());
    assert_eq!(app.canvas_held_bytes(), 79_524 + 80_000);
}

#[test]
fn landing_asks_again_when_room_grew_for_a_refused_picture() {
    let (mut app, _dir) = new_app("cvi_ask_room");
    let key: Arc<str> = Arc::from("canvas://pa/0/00000000000000c1");
    let pics = |needs: u64| CanvasPictures {
        scope_fp: 0,
        places: vec![CanvasPlace {
            id: 0,
            rect: CellRect::new(0, 0, 1, 1),
            key: Some(key.clone()),
        }],
        fresh: Vec::new(),
        waiting: 0,
        refused: 1,
        more: false,
        sources: vec![(
            0,
            KeptPicture {
                key: Some(key.clone()),
                hash: Some(0xc1),
                data: Arc::new(rgba(100, 100, 5)),
                needs: Some(needs),
            },
        )],
    };
    assert!(
        app.canvas_pictures_landed_within(pics(40_000), 160_000),
        "the cache holds nothing: it fits now, ask for the answer that decodes it"
    );
    assert!(
        !app.canvas_pictures_landed_within(pics(200_000), 160_000),
        "it can not fit: asking would not help"
    );
}

#[test]
fn pictures_left_unhashed_ask_for_another_answer_though_this_one_decoded_nothing() {
    let (mut app, _dir) = new_app("cvi_unhashed_more");
    let l = small(|l| l.input_bytes = 10_000);
    let bg = rgba(100, 100, 31); // 40,000 bytes of input: over the budget by itself
    let first = decode_within(&canvas_of(vec![im(1, bg.clone())]), &nothing_held(), l);
    assert_eq!(first.fresh.len(), 1);
    app.canvas_pictures_landed(first);
    // The background is sent again (allowed: `unchanged` is a hint) with a new small picture.
    let again = decode_within(
        &canvas_of(vec![im(1, bg), im(2, rgba(2, 2, 32))]),
        &app.canvas_known_keys_for_test(),
        l,
    );
    assert!(again.fresh.is_empty(), "the background is held");
    assert_eq!(again.waiting, 1, "the icon was not even hashed");
    assert!(again.more, "the next answer takes the kept icon first");
    assert!(
        app.canvas_pictures_landed(again),
        "asked for, though nothing was added by this landing"
    );
    let next = decode_within(
        &canvas_of(vec![none(1), none(2)]),
        &app.canvas_known_keys_for_test(),
        l,
    );
    assert_eq!(next.fresh.len(), 1, "the icon comes with the next answer");
    app.canvas_pictures_landed(next);
    assert!(app.canvas_kept.is_empty());
}

#[test]
fn the_kept_sources_are_bounded_and_the_oldest_go_first() {
    let (mut app, _dir) = new_app("cvi_kept_bound");
    let l = small(|l| l.decodes = 1);
    let cap = 90_000; // two 40,000-byte sources fit, three do not
    let pic = |i: u64| im(i, rgba(100, 100, 100 + i as u8));
    // One decoded, three deferred (120,000 bytes): the first deferred goes.
    let got = decode_within(&canvas_of((0..4).map(pic).collect()), &nothing_held(), l);
    assert_eq!(got.sources.len(), 4);
    app.canvas_pictures_landed_limits(got, MAX_CANVAS_HELD_BYTES, cap);
    let mut ids: Vec<u64> = app.canvas_kept.keys().copied().collect();
    ids.sort();
    assert_eq!(ids, [2, 3], "id 1 was the first deferred, and goes");
    // Two new ones are deferred behind them: now it is the older kept (id 3) that goes.
    let got = decode_within(
        &canvas_of(vec![none(2), none(3), pic(4), pic(5)]),
        &app.canvas_known_keys_for_test(),
        l,
    );
    app.canvas_pictures_landed_limits(got, MAX_CANVAS_HELD_BYTES, cap);
    let mut ids: Vec<u64> = app.canvas_kept.keys().copied().collect();
    ids.sort();
    assert_eq!(
        ids,
        [4, 5],
        "the kept sources are over the bound: the oldest go"
    );
    // A dropped source shows its cells until the plugin sends the picture again.
    let again = decode_within(
        &canvas_of(vec![none(3)]),
        &app.canvas_known_keys_for_test(),
        l,
    );
    assert!(again.fresh.is_empty());
    assert!(!app.md_images.cache.contains_key(Path::new(
        &*again.places[0].key.clone().unwrap_or_else(|| Arc::from(""))
    )));
}

#[test]
fn the_oldest_kept_source_goes_first_whatever_the_order_the_worker_listed_them_in() {
    let (mut app, _dir) = new_app("cvi_kept_old");
    let kp = |c: u8| KeptPicture {
        key: None,
        hash: None,
        data: Arc::new(rgba(100, 100, c)),
        needs: None,
    };
    // Id 2 was kept by an earlier landing.
    app.canvas_kept.insert(2, kp(1));
    // This answer lists a new unhashed source (id 5) before the old one: sources are listed by the
    // pass that made them, not by age.
    let places = [5u64, 2, 6]
        .iter()
        .map(|id| CanvasPlace {
            id: *id,
            rect: CellRect::new(0, 0, 1, 1),
            key: None,
        })
        .collect();
    let pics = CanvasPictures {
        scope_fp: 0,
        places,
        fresh: Vec::new(),
        waiting: 3,
        refused: 0,
        more: false,
        sources: vec![(5, kp(2)), (2, kp(3)), (6, kp(4))],
    };
    app.canvas_pictures_landed_limits(pics, MAX_CANVAS_HELD_BYTES, 90_000);
    let mut ids: Vec<u64> = app.canvas_kept.keys().copied().collect();
    ids.sort();
    assert_eq!(ids, [5, 6], "the one kept longest (id 2) is dropped");
}

// ---- round 3 of the mutation audit: survivors of the pictures' worker and landing --------------------

#[test]
fn pictures_are_drawable_only_with_an_image_backend_and_the_encode_worker() {
    let (mut app, _dir) = new_app("cvi_drawable");
    assert!(!app.canvas_pictures_drawable());
    app.picker = Some(ratatui_image::picker::Picker::halfblocks());
    assert!(app.md_images.enc_tx.is_none());
    assert!(
        !app.canvas_pictures_drawable(),
        "a picker without the encode worker"
    );
    assert!(app.canvas_known_keys().is_none());
    let (tx, _rx) = std::sync::mpsc::channel();
    app.md_images.enc_tx = Some(tx);
    assert!(app.canvas_pictures_drawable());
    assert!(app.canvas_known_keys().is_some());
    app.picker = None;
    assert!(
        !app.canvas_pictures_drawable(),
        "the encode worker without a picker"
    );
}

#[test]
fn a_decoded_picture_for_a_key_already_held_leaves_the_entry_as_it_is() {
    let (mut app, _dir) = new_app("cvi_double_add");
    let first = decode(
        "pdbl",
        &canvas_of(vec![im(1, rgba(4, 4, 1))]),
        &nothing_held(),
    );
    let key = first.fresh[0].0.clone();
    let places = first.places.clone();
    app.canvas_pictures_landed(first);
    let before = app.md_images.cache[&key].pixel_bytes();
    assert_eq!(before, 64);
    // A job that was out before that landing answers with the same key (decoded bigger here, to
    // see whether it replaces the entry).
    let second = CanvasPictures {
        scope_fp: 0,
        places,
        fresh: vec![(
            key.clone(),
            CanvasDecoded::Still(image::DynamicImage::new_rgba8(8, 8)),
        )],
        waiting: 0,
        refused: 0,
        more: false,
        sources: Vec::new(),
    };
    app.canvas_pictures_landed(second);
    assert_eq!(app.md_images.cache.len(), 1);
    assert_eq!(
        app.md_images.cache[&key].pixel_bytes(),
        before,
        "the entry (with its encodes and kitty ids) is kept"
    );
}

#[test]
fn the_old_picture_of_an_id_stays_when_the_new_content_exactly_fits_next_to_it() {
    let (mut app, _dir) = new_app("cvi_exact_front");
    let old = decode(
        "pef",
        &canvas_of(vec![im(1, rgba(100, 100, 1))]),
        &nothing_held(),
    );
    let old_key = old.places[0].key.clone().unwrap();
    app.canvas_pictures_landed_within(old, 80_000);
    app.canvas_front.insert(1, old_key.clone());
    let held = app.canvas_known_keys_for_test();
    let new = decode_within(
        &canvas_of(vec![im(1, rgba(100, 100, 2))]),
        &held,
        small(|l| l.held_bytes = 80_000),
    );
    assert_eq!(new.fresh.len(), 1);
    app.canvas_pictures_landed_within(new, 80_000);
    // 40,000 + 40,000 is exactly the bound: it fits, so nothing had to give way (the old picture
    // is still drawn until the new one can be).
    assert_eq!(app.md_images.cache.len(), 2);
    assert_eq!(app.canvas_front.get(&1), Some(&old_key));
}

#[test]
fn landing_asks_again_when_the_room_is_exactly_what_a_refused_picture_needs() {
    let (mut app, _dir) = new_app("cvi_exact_room");
    let kp = |needs: u64| {
        (
            1u64,
            KeptPicture {
                key: Some(Arc::from("canvas://pw/1/00000000000000aa")),
                hash: Some(0xaa),
                data: Arc::new(rgba(100, 100, 1)),
                needs: Some(needs),
            },
        )
    };
    let pics = |needs: u64| CanvasPictures {
        scope_fp: 0,
        places: vec![CanvasPlace {
            id: 1,
            rect: CellRect::new(0, 0, 2, 2),
            key: Some(Arc::from("canvas://pw/1/00000000000000aa")),
        }],
        fresh: Vec::new(),
        waiting: 0,
        refused: 1,
        more: false,
        sources: vec![kp(needs)],
    };
    assert!(app.canvas_pictures_landed_within(pics(40_000), 40_000));
    assert!(!app.canvas_pictures_landed_within(pics(40_001), 40_000));
}

#[test]
fn kept_sources_exactly_at_their_bound_are_all_kept() {
    let (mut app, _dir) = new_app("cvi_kept_exact");
    let kp = |c: u8| KeptPicture {
        key: None,
        hash: None,
        data: Arc::new(rgba(100, 100, c)), // 40,000 bytes of source
        needs: None,
    };
    let places = [1u64, 2]
        .iter()
        .map(|id| CanvasPlace {
            id: *id,
            rect: CellRect::new(0, 0, 1, 1),
            key: None,
        })
        .collect();
    let pics = CanvasPictures {
        scope_fp: 0,
        places,
        fresh: Vec::new(),
        waiting: 2,
        refused: 0,
        more: false,
        sources: vec![(1, kp(1)), (2, kp(2))],
    };
    app.canvas_pictures_landed_limits(pics, MAX_CANVAS_HELD_BYTES, 80_000);
    let mut ids: Vec<u64> = app.canvas_kept.keys().copied().collect();
    ids.sort();
    assert_eq!(ids, [1, 2], "80,000 of 80,000 is within the bound");
}

// ---- the kept sources are shared, not copied; frames are decodes (stage 2.5 / part 2) ------------

fn animation(n: usize, c: u8) -> ImageData {
    ImageData::Frames(
        (0..n)
            .map(|i| {
                (
                    rgba(1, 1, c.wrapping_add(i as u8)),
                    Duration::from_millis(100),
                )
            })
            .collect(),
    )
}

#[test]
fn the_table_a_job_gets_shares_the_kept_data_instead_of_copying_it() {
    // A thousand-frame animation kept for later: the copy of the table that goes with every job
    // (Open, View, Act, Keys) holds the very same allocation, so the UI thread copies pointers,
    // not frames.
    let (mut app, _dir) = new_app("cvi_share");
    app.picker = Some(ratatui_image::picker::Picker::halfblocks());
    let (tx, _rx) = std::sync::mpsc::channel();
    app.attach_md_encoder(tx);
    let data = Arc::new(animation(1000, 1));
    app.canvas_kept.insert(
        7,
        KeptPicture {
            key: None,
            hash: None,
            data: data.clone(),
            needs: None,
        },
    );
    let held = app.canvas_known_keys().expect("pictures can be shown");
    assert!(
        Arc::ptr_eq(&held.kept[&7].data, &data),
        "the job's table points at the kept data"
    );
    // A second job: still the one allocation (3 holders: the test, the app, the two copies).
    let again = app.canvas_known_keys().unwrap();
    assert!(Arc::ptr_eq(&again.kept[&7].data, &held.kept[&7].data));
    assert_eq!(Arc::strong_count(&data), 4);
}

#[test]
fn landing_frees_the_replaced_kept_table_off_the_ui_thread() {
    let (mut app, _dir) = new_app("cvi_discard");
    for id in 1..=3u64 {
        app.canvas_kept.insert(
            id,
            KeptPicture {
                key: None,
                hash: None,
                data: Arc::new(animation(10, id as u8)),
                needs: None,
            },
        );
    }
    super::table_actions::DISCARDED.with(|d| d.borrow_mut().clear());
    app.canvas_pictures_landed(decode("pd", &canvas_of(vec![]), &nothing_held()));
    assert!(app.canvas_kept.is_empty());
    let discarded = super::table_actions::DISCARDED.with(|d| d.borrow().clone());
    assert!(
        discarded.iter().any(|t| t.contains("KeptPicture")),
        "the old table was handed to the reaper, not dropped here: {discarded:?}"
    );
}

#[test]
fn every_svg_frame_of_an_animation_is_a_decode_of_the_answer() {
    // decodes = 4: a 3-frame SVG animation takes 3; the next 3-frame one does not fit next to it and
    // waits (kept, not failed); a still after it takes the last one.
    let c = canvas_of(vec![
        im(1, svg_animation_seeded(3, 10)),
        im(2, svg_animation_seeded(3, 50)),
        im(3, rgba(1, 1, 99)),
    ]);
    let got = decode_within(&c, &nothing_held(), small(|l| l.decodes = 4));
    let keys_done: Vec<&str> = got.fresh.iter().map(|(k, _)| k.to_str().unwrap()).collect();
    assert_eq!(got.fresh.len(), 2, "{keys_done:?}");
    assert!(keys_done.iter().any(|k| k.starts_with("canvas://pw/1/")));
    assert!(keys_done.iter().any(|k| k.starts_with("canvas://pw/3/")));
    assert!(
        got.fresh
            .iter()
            .all(|(_, d)| !matches!(d, CanvasDecoded::Failed(_))),
        "the one that waits did not fail"
    );
    assert_eq!(got.waiting, 1);
    assert!(got.more);
    let (id, kept) = got
        .sources
        .iter()
        .find(|(_, k)| {
            k.needs.is_none()
                && k.key
                    .as_deref()
                    .is_some_and(|k| k.starts_with("canvas://pw/2/"))
        })
        .expect("the waiting animation is kept");
    assert_eq!(*id, 2);
    // The next answer (the kept ones go first) decodes it.
    let mut held = held_after(&nothing_held(), &got);
    held.kept.insert(*id, kept.clone());
    let next = decode_within(&canvas_of(vec![none(2)]), &held, small(|l| l.decodes = 4));
    assert_eq!(next.fresh.len(), 1);
    assert!(matches!(next.fresh[0].1, CanvasDecoded::Frames(_)));
}

/// An animation of `n` SVG frames (each is a child-process decode).
fn svg_animation(n: usize) -> ImageData {
    svg_animation_seeded(n, 0)
}

fn svg_animation_seeded(n: usize, seed: u8) -> ImageData {
    ImageData::Frames(
        (0..n)
            .map(|i| {
                (
                    ImageData::Svg {
                        data: Arc::from(
                            format!(
                                "<svg xmlns='http://www.w3.org/2000/svg' width='4' height='4'><rect width='4' height='4' fill='#{:02x}0000'/></svg>",
                                (i as u8).wrapping_add(seed)
                            )
                            .into_bytes(),
                        ),
                        base_dir: None,
                        trusted: false,
                    },
                    Duration::from_millis(100),
                )
            })
            .collect(),
    )
}

#[test]
fn a_raster_animation_of_many_frames_is_decoded_and_svg_frames_count_against_the_budget() {
    // Raster frames are cheap: a 100-frame animation decodes in one answer although the budget is
    // 4 decodes; only its input bytes are bounded.
    let c = canvas_of(vec![im(1, animation(100, 1)), im(2, rgba(1, 1, 7))]);
    let got = decode_within(&c, &nothing_held(), small(|l| l.decodes = 4));
    assert_eq!(got.fresh.len(), 2, "raster frames cost no decode");
    assert!(got
        .fresh
        .iter()
        .any(|(_, d)| matches!(d, CanvasDecoded::Frames(f) if f.len() == 100)));
    // SVG frames each are a decode: more of them than the budget is refused for good, wherever
    // it stands, and never run as 40 child decodes in one answer.
    for order in [true, false] {
        let mut v = vec![im(1, svg_animation(40)), im(2, rgba(1, 1, 7))];
        if !order {
            v.reverse();
        }
        let got = decode_within(&canvas_of(v), &nothing_held(), small(|l| l.decodes = 32));
        let key = got
            .places
            .iter()
            .find(|p| p.id == 1)
            .unwrap()
            .key
            .clone()
            .unwrap();
        let (_, d) = got
            .fresh
            .iter()
            .find(|(k, _)| k.to_str() == Some(&*key))
            .unwrap();
        assert!(matches!(
            d,
            CanvasDecoded::Failed(crate::preview::image::ImageFailure::TooLarge)
        ));
        assert_eq!(got.waiting, 0);
    }
    // One that fits the budget by itself runs when first and waits behind what used the budget.
    let fits = canvas_of(vec![im(1, svg_animation(4)), im(2, rgba(1, 1, 7))]);
    let got = decode_within(&fits, &nothing_held(), small(|l| l.decodes = 4));
    assert!(got
        .fresh
        .iter()
        .any(|(_, d)| matches!(d, CanvasDecoded::Frames(_))));
    assert_eq!(got.waiting, 1);
    let behind = canvas_of(vec![im(2, rgba(1, 1, 7)), im(1, svg_animation(4))]);
    let got = decode_within(&behind, &nothing_held(), small(|l| l.decodes = 4));
    assert_eq!(got.fresh.len(), 1, "the still first, the animation waits");
    assert_eq!(got.waiting, 1);
}

// ---- leaving the program deletes the canvas pictures from the terminal (S6) --------------------

#[test]
fn the_exit_cleanup_writes_a_delete_for_every_canvas_picture_still_placed() {
    let table = LiveKittyIds::new();
    for id in [7, 3, 9] {
        table.note(id);
    }
    table.forget(3); // already deleted by the sweep
    let mut out = Vec::new();
    delete_kitty_images_in(&table, &mut out);
    let wire = String::from_utf8(out).unwrap();
    assert_eq!(
        wire,
        format!(
            "{}{}",
            crate::preview::kitty::build_delete(7, false),
            crate::preview::kitty::build_delete(9, false)
        ),
        "one delete per id still placed, none for the one already gone"
    );
    // Taken: a second exit step writes nothing.
    let mut again = Vec::new();
    delete_kitty_images_in(&table, &mut again);
    assert!(again.is_empty());
}

#[test]
fn the_exit_cleanup_wraps_the_deletes_for_tmux() {
    let table = LiveKittyIds::new();
    table.set_tmux(true);
    table.note(4);
    let mut out = Vec::new();
    delete_kitty_images_in(&table, &mut out);
    assert_eq!(
        String::from_utf8(out).unwrap(),
        crate::preview::kitty::build_delete(4, true)
    );
}

#[test]
fn an_empty_table_writes_nothing_and_does_not_flush() {
    struct NoFlush(Vec<u8>);
    impl std::io::Write for NoFlush {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.0.extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            panic!("flushed with nothing to write");
        }
    }
    let mut w = NoFlush(Vec::new());
    delete_kitty_images_in(&LiveKittyIds::new(), &mut w);
    assert!(w.0.is_empty());
}

#[test]
fn a_write_that_fails_does_not_panic() {
    struct Gone;
    impl std::io::Write for Gone {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("the terminal is gone"))
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Err(std::io::Error::other("the terminal is gone"))
        }
    }
    let table = LiveKittyIds::new();
    table.note(1);
    delete_kitty_images_in(&table, &mut Gone);
}

#[test]
fn a_canvas_picture_given_a_kitty_id_is_noted_for_the_exit_and_other_pictures_are_not() {
    use super::md_media::kitty_id_for_test;
    let mut ids = HashMap::new();
    let full = MdEncodeKey::Full { cols: 2, rows: 2 };
    let canvas = Path::new("canvas://pexit/1/00000000000000aa");
    let id = kitty_id_for_test(&mut ids, canvas, &full, 0, true).expect("an id");
    assert!(LIVE_CANVAS_KITTY.snapshot().contains(&id), "noted");
    let other = kitty_id_for_test(&mut ids, Path::new("/x/a.png"), &full, 0, true).expect("an id");
    assert!(
        !LIVE_CANVAS_KITTY.snapshot().contains(&other),
        "a Markdown picture is not a canvas picture"
    );
    // Not on a terminal without kitty graphics: no id, nothing noted.
    assert_eq!(
        kitty_id_for_test(&mut ids, Path::new("canvas://pexit/2/00"), &full, 0, false),
        None
    );
}

#[test]
fn the_ids_whose_deletes_were_handed_out_are_forgotten() {
    let (mut app, _dir) = new_app("cvi_exit_forget");
    app.md_images.use_kitty = true;
    let id = crate::preview::kitty::next_id();
    LIVE_CANVAS_KITTY.note(id);
    app.md_images.kitty_deletes = vec![id];
    assert_eq!(app.take_kitty_deletes().len(), 1);
    assert!(
        !LIVE_CANVAS_KITTY.snapshot().contains(&id),
        "the exit does not delete it a second time"
    );
}

// ---- picture files (S7) ------------------------------------------------------------------------

#[test]
fn a_canvas_pictures_encode_slots_never_grow_past_the_cap() {
    use crate::app::{reserve_proto_slot_hard, MdEncodeKey, MD_PROTO_SLOTS};
    let key = |n: u16| MdEncodeKey::Full { cols: n, rows: 1 };
    let mut slots = Vec::new();
    // One frame wants more keys than the family can hold: it is held at the cap, the rest get no
    // slot (their cells stay), where a document's family would grow by one each.
    let mut given = 0;
    for n in 1..=50u16 {
        if reserve_proto_slot_hard(&mut slots, key(n), 10, MD_PROTO_SLOTS).is_some() {
            given += 1;
        }
    }
    assert_eq!(given, MD_PROTO_SLOTS);
    assert_eq!(slots.len(), MD_PROTO_SLOTS);
    // The next frame recycles the ones the last frame did not want, still at the cap.
    for n in 100..=150u16 {
        let _ = reserve_proto_slot_hard(&mut slots, key(n), 11, MD_PROTO_SLOTS);
    }
    assert_eq!(slots.len(), MD_PROTO_SLOTS);
    // A key that is already there is found, not duplicated.
    let first = slots[0].key;
    let again = reserve_proto_slot_hard(&mut slots, first, 11, MD_PROTO_SLOTS);
    assert_eq!(again, Some(0));
    assert_eq!(slots.len(), MD_PROTO_SLOTS);
}

// ---- the read scope of a plugin the user added (canvas pictures) ------------------------------------

// ---- read scope ------------------------------------------------------------------------------------

fn scope_files(name: &str) -> (crate::test_support::TmpDir, PathBuf, PathBuf) {
    let dir = crate::test_support::unique_tmp(name);
    let (inside, outside) = (dir.join("in"), dir.join("out"));
    std::fs::create_dir_all(&inside).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    let (a, b) = (inside.join("a.png"), outside.join("b.png"));
    for p in [&a, &b] {
        image::RgbaImage::from_pixel(3, 3, image::Rgba([1, 2, 3, 255]))
            .save(p)
            .unwrap();
    }
    (dir, a, b)
}

fn decode_scoped(
    c: &CanvasView,
    held: &CanvasHeld,
    scope: &konoma_plugin::ReadScope,
) -> CanvasPictures {
    dc("demo", c, held, Path::new("/x/a.spr"), scope)
}

fn is_outside(p: &CanvasPictures) -> bool {
    p.fresh.iter().any(|(_, d)| {
        matches!(
            d,
            CanvasDecoded::Failed(crate::preview::image::ImageFailure::OutsideReadDirs)
        )
    })
}

#[test]
fn a_picture_judged_under_one_scope_is_not_served_to_another() {
    let (_dir, a, _b) = scope_files("cvi_scope_switch");
    let c = canvas_of(vec![im(1, ImageData::File(a.clone()))]);
    let wide = konoma_plugin::ReadScope::within([a.parent().unwrap()]);
    let narrow =
        konoma_plugin::ReadScope::within([a.parent().unwrap().parent().unwrap().join("out")]);
    let (mut app, _d) = new_app("cvi_scope_switch_app");
    let first = decode_scoped(&c, &nothing_held(), &wide);
    assert!(matches!(first.fresh[0].1, CanvasDecoded::Still(_)));
    app.canvas_pictures_landed(first);
    // The same file, the same content, but the scope no longer holds it: the held key of the
    // first answer must not be reused.
    let held = app.canvas_known_keys_for_test();
    let second = decode_scoped(&c, &held, &narrow);
    assert!(is_outside(&second), "refused, not served from the cache");
    assert_ne!(keys(&second)[0], None);
}

#[test]
fn a_refused_picture_flashes_the_reason_once_per_plugin() {
    let (_dir, a, _b) = scope_files("cvi_flash");
    let c = canvas_of(vec![im(1, ImageData::File(a.clone()))]);
    let none_ok = konoma_plugin::ReadScope::within(Vec::<PathBuf>::new());
    let pics = decode_scoped(&c, &nothing_held(), &none_ok);
    assert!(is_outside(&pics));
    let (mut app, _d) = new_app("cvi_flash_app");
    app.canvas_flash_refusals("demo", &pics);
    let msg = app.flash.clone().expect("flashed");
    assert!(msg.contains("read_dirs") && msg.contains("demo"), "{msg}");
    assert!(!msg.contains("<id>"), "{msg}");
    // Not again for the same plugin.
    app.flash = None;
    app.canvas_flash_refusals("demo", &pics);
    assert!(app.flash.is_none(), "no re-flash");
    // Another plugin is told.
    app.canvas_flash_refusals("other", &pics);
    assert!(app.flash.as_deref().is_some_and(|m| m.contains("other")));
    // An answer without a refusal flashes nothing.
    let (mut app2, _d2) = new_app("cvi_flash_ok");
    let ok = decode_scoped(
        &c,
        &nothing_held(),
        &konoma_plugin::ReadScope::unrestricted(),
    );
    app2.canvas_flash_refusals("demo", &ok);
    assert!(app2.flash.is_none());
}

#[test]
fn the_refusal_text_names_the_plugin_in_both_languages() {
    use crate::i18n::Lang;
    use crate::preview::image::{ImageFailure, ImageFailureMessage};
    for lang in [Lang::En, Lang::Jp] {
        let t = ImageFailure::OutsideReadDirs.message(lang, "demo").unwrap();
        assert!(
            t.contains("[plugins.demo]") && t.contains("read_dirs"),
            "{t}"
        );
        assert!(!t.contains("<id>"), "{t}");
        // Other reasons are untouched.
        let o = ImageFailure::Corrupt.message(lang, "demo").unwrap();
        assert!(!o.contains("demo"));
    }
}

#[test]
fn an_svg_file_in_scope_cannot_read_a_file_outside_through_its_href() {
    // A canvas picture `ImageData::File(in-scope evil.svg)` whose SVG names an absolute file
    // outside the scope: the host reads the SVG (allowed) but not what it points at.
    let dir = crate::test_support::unique_tmp("konoma_canvas_svg_href");
    let base = dir.canonicalize().unwrap_or(dir.to_path_buf());
    let (root, elsewhere) = (base.join("root"), base.join("elsewhere"));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&elsewhere).unwrap();
    let secret = elsewhere.join("secret.png");
    image::RgbaImage::from_pixel(20, 20, image::Rgba([255, 0, 0, 255]))
        .save(&secret)
        .unwrap();
    let evil = root.join("evil.svg");
    std::fs::write(
        &evil,
        format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="20" height="20"><image xlink:href="{}" width="20" height="20"/></svg>"#,
            secret.display()
        ),
    )
    .unwrap();
    let shown = root.join("a.spr");
    let alpha = |scope: &konoma_plugin::ReadScope| match decode_one(
        &ImageData::File(evil.clone()),
        &shown,
        &konoma_plugin::canvas_pictures::CanvasPolicy::new(
            scope,
            &super::super::media_plugin::SvgChild,
            64,
        ),
    ) {
        CanvasDecoded::Still(img) => {
            let px = img.to_rgba8();
            px.get_pixel(px.width() / 2, px.height() / 2)[3]
        }
        _ => panic!("the SVG is drawn"),
    };
    let scope = konoma_plugin::ReadScope::within([root.as_path()]);
    assert_eq!(alpha(&scope), 0, "the file outside is not read");
    // The same file as a frame of an animation goes through the same judgement.
    let framed = decode_one(
        &ImageData::Frames(vec![(
            ImageData::File(evil.clone()),
            std::time::Duration::from_millis(50),
        )]),
        &shown,
        &konoma_plugin::canvas_pictures::CanvasPolicy::new(
            &scope,
            &super::super::media_plugin::SvgChild,
            64,
        ),
    );
    match framed {
        CanvasDecoded::Frames(f) => {
            let px = f[0].0.to_rgba8();
            assert_eq!(
                px.get_pixel(px.width() / 2, px.height() / 2)[3],
                0,
                "a frame"
            );
        }
        _ => panic!("the frames are drawn"),
    }
    assert_eq!(
        alpha(&konoma_plugin::ReadScope::unrestricted()),
        255,
        "unrestricted reads it"
    );
}

// ---- the per-answer budget counts the header pixels before anything is decoded ----

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in bytes {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind {
    Png,
    Jpeg,
    Gif,
    Webp,
}

/// A file of a few dozen bytes whose header announces `w` x `h` pixels (a real 1-pixel picture
/// with the size fields patched; the pixel data is not there to decode). `salt` makes the content
/// (and so the hash) differ between two files of the same size.
fn huge_header(kind: Kind, w: u32, h: u32, salt: u8) -> Vec<u8> {
    use image::ImageEncoder;
    match kind {
        Kind::Png => {
            let mut out = Vec::new();
            image::codecs::png::PngEncoder::new(&mut out)
                .write_image(&[salt, 0, 0], 1, 1, image::ExtendedColorType::Rgb8)
                .unwrap();
            out[16..20].copy_from_slice(&w.to_be_bytes());
            out[20..24].copy_from_slice(&h.to_be_bytes());
            let crc = crc32(&out[12..29]);
            out[29..33].copy_from_slice(&crc.to_be_bytes());
            out
        }
        Kind::Jpeg => {
            let mut out = Vec::new();
            image::codecs::jpeg::JpegEncoder::new(&mut out)
                .write_image(&[salt; 8 * 8 * 3], 8, 8, image::ExtendedColorType::Rgb8)
                .unwrap();
            let sof = out
                .windows(2)
                .position(|p| p == [0xFF, 0xC0])
                .expect("a baseline frame header");
            // marker(2) length(2) precision(1) height(2) width(2)
            out[sof + 5..sof + 7].copy_from_slice(&(h as u16).to_be_bytes());
            out[sof + 7..sof + 9].copy_from_slice(&(w as u16).to_be_bytes());
            out
        }
        Kind::Gif => {
            let mut out = b"GIF89a".to_vec();
            out.extend_from_slice(&(w as u16).to_le_bytes());
            out.extend_from_slice(&(h as u16).to_le_bytes());
            out.extend_from_slice(&[0, salt, 0]);
            // One 1 x 1 frame (descriptor, LZW minimum size 2, one data block), then the trailer.
            out.extend_from_slice(&[0x2C, 0, 0, 0, 0, 1, 0, 1, 0, 0, 2, 2, 0x4C, 0x01, 0, 0x3B]);
            out
        }
        Kind::Webp => {
            // A real 1 x 1 lossless picture with the 14-bit size fields of its VP8L header
            // patched (its one-symbol codes hold for any size, so the decoder starts).
            let mut out = Vec::new();
            image::codecs::webp::WebPEncoder::new_lossless(&mut out)
                .write_image(&[salt, 0, 0, 255], 1, 1, image::ExtendedColorType::Rgba8)
                .unwrap();
            let at = out.windows(4).position(|p| p == b"VP8L").unwrap() + 8;
            assert_eq!(out[at], 0x2F);
            let old = u32::from_le_bytes([out[at + 1], out[at + 2], out[at + 3], out[at + 4]]);
            let packed = ((w - 1) | ((h - 1) << 14)) | (old & 0xF000_0000);
            out[at + 1..at + 5].copy_from_slice(&packed.to_le_bytes());
            out
        }
    }
}

const KINDS: [Kind; 4] = [Kind::Png, Kind::Jpeg, Kind::Gif, Kind::Webp];

fn encoded(bytes: Vec<u8>) -> ImageData {
    ImageData::Encoded(Arc::from(bytes))
}

#[test]
fn the_header_pixels_of_a_file_are_read_through_the_regular_file_door() {
    let dir = crate::test_support::unique_tmp("cvi_header_px_file");
    std::fs::create_dir_all(&dir).unwrap();
    for kind in KINDS {
        let (w, h) = (3000, 2000);
        let path = dir.join(format!("p_{kind:?}.bin"));
        std::fs::write(&path, huge_header(kind, w, h, 2)).unwrap();
        assert_eq!(
            header_pixels(
                &ImageData::File(path.clone()),
                &dir.join("a.spr"),
                &konoma_plugin::ReadScope::unrestricted()
            ),
            (6_000_000, false),
            "{kind:?}"
        );
    }
    // An SVG file is not counted by a header: its cost is known once it is drawn.
    let svg = dir.join("s.svg");
    std::fs::write(
        &svg,
        b"<svg xmlns='http://www.w3.org/2000/svg' width='9' height='9'/>",
    )
    .unwrap();
    assert_eq!(
        header_pixels(
            &ImageData::File(svg),
            &dir.join("a.spr"),
            &konoma_plugin::ReadScope::unrestricted()
        ),
        (0, true)
    );
    // A missing file costs nothing here (the decode says why it cannot be shown).
    assert_eq!(
        header_pixels(
            &ImageData::File(dir.join("none.png")),
            &dir.join("a.spr"),
            &konoma_plugin::ReadScope::unrestricted()
        ),
        (0, false)
    );
    // An animation costs the sum of its frames.
    let frames = ImageData::Frames(vec![
        (
            encoded(huge_header(Kind::Png, 100, 100, 1)),
            Duration::from_millis(50),
        ),
        (
            encoded(huge_header(Kind::Png, 200, 100, 2)),
            Duration::from_millis(50),
        ),
    ]);
    assert_eq!(
        header_pixels(
            &frames,
            &dir.join("a.spr"),
            &konoma_plugin::ReadScope::unrestricted()
        ),
        (30_000, false)
    );
}

#[test]
fn an_svg_is_counted_by_the_pixels_it_is_drawn_to() {
    // No header to read: the drawing is the work. The first SVG is drawn (a first picture always
    // runs) and its pixels count; the second does not fit next to it and waits.
    let svg = |n: u32| ImageData::Svg {
        data: Arc::from(
            format!(
                "<svg xmlns='http://www.w3.org/2000/svg' width='40' height='{}'/>",
                40 + n
            )
            .into_bytes(),
        ),
        base_dir: None,
        trusted: true,
    };
    let c = canvas_of(vec![im(1, svg(1)), im(2, svg(2)), im(3, svg(3))]);
    let got = run(
        "pw",
        &c,
        &nothing_held(),
        Path::new("/x/a.spr"),
        &konoma_plugin::ReadScope::unrestricted(),
        small(|l| l.header_pixels = 100),
        None,
        true,
    );
    assert_eq!(got.fresh.len(), 1, "the first drew and used the budget");
    assert_eq!(got.waiting, 2);
    assert!(got.more);
    // With room for all of them, all are drawn.
    let all = run(
        "pw",
        &c,
        &nothing_held(),
        Path::new("/x/a.spr"),
        &konoma_plugin::ReadScope::unrestricted(),
        CanvasLimits::PRODUCTION,
        None,
        true,
    );
    assert_eq!(all.fresh.len(), 3);
}

#[test]
fn an_svg_frame_inside_an_animation_is_counted_by_the_pixels_it_is_drawn_to() {
    let svg = |n: u32| ImageData::Svg {
        data: Arc::from(
            format!(
                "<svg xmlns='http://www.w3.org/2000/svg' width='40' height='{}'/>",
                40 + n
            )
            .into_bytes(),
        ),
        base_dir: None,
        trusted: true,
    };
    let anim = |n: u32| ImageData::Frames(vec![(svg(n), Duration::from_millis(50))]);
    // The header of an animation with an SVG frame is unknown before drawing, like a bare SVG.
    assert_eq!(
        header_pixels(
            &anim(1),
            Path::new("/x/a.spr"),
            &konoma_plugin::ReadScope::unrestricted()
        ),
        (0, true)
    );
    // The first animation draws and its pixels count; the second does not fit next to it.
    let c = canvas_of(vec![im(1, anim(1)), im(2, anim(2))]);
    let got = run(
        "pw",
        &c,
        &nothing_held(),
        Path::new("/x/a.spr"),
        &konoma_plugin::ReadScope::unrestricted(),
        small(|l| l.header_pixels = 100),
        None,
        true,
    );
    assert_eq!(got.fresh.len(), 1, "the first drew and used the budget");
    assert_eq!(got.waiting, 1);
    assert!(got.more);
}
