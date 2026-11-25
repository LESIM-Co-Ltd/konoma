//! The worker side of canvas pictures: keys, hashes, what is decoded and what is skipped, limits;
//! and the landing of an answer in the cache.

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
        .canvas
        .places
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
    assert!(app.canvas.places.is_empty());
    // And forgetting clears whatever is left.
    app.canvas_pictures_landed(decode("pl", &c1, &nothing_held()));
    app.forget_canvas_pictures();
    assert!(app.canvas.places.is_empty());
    assert!(app
        .md_images
        .cache
        .keys()
        .all(|k| !crate::preview::markdown::is_canvas_url(&k.to_string_lossy())));
}

#[test]
fn no_known_keys_without_an_image_backend() {
    let (app, _dir) = new_app("cvi_nobackend");
    assert!(app.canvas_known_keys().is_none());
    assert!(!app.canvas_pictures_drawable());
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
    app.canvas.kept.insert(
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
