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

fn decode(plugin: &str, c: &CanvasView, held: &CanvasHeld) -> CanvasPictures {
    decode_canvas(plugin, c, held, Path::new("/x/a.spr"), 800, None, false)
}

fn small(limits: impl FnOnce(&mut CanvasLimits)) -> CanvasLimits {
    let mut l = CanvasLimits::PRODUCTION;
    limits(&mut l);
    l
}

fn decode_within(c: &CanvasView, held: &CanvasHeld, l: CanvasLimits) -> CanvasPictures {
    decode_canvas_within("pw", c, held, Path::new("/x/a.spr"), 800, None, false, l)
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

// ---- keys and hashes ----------------------------------------------------------------------------

#[test]
fn key_is_scoped_to_the_plugin_and_the_image_id_and_follows_the_content() {
    let c = canvas_of(vec![im(1, rgba(4, 4, 10))]);
    let a = keys(&decode("pa", &c, &nothing_held()));
    let a2 = keys(&decode("pa", &c, &nothing_held()));
    let b = keys(&decode("pb", &c, &nothing_held()));
    assert_eq!(a, a2, "same plugin, id and content: the same key");
    assert_ne!(a, b, "another plugin: another key");
    let other_id = canvas_of(vec![im(2, rgba(4, 4, 10))]);
    assert_ne!(a, keys(&decode("pa", &other_id, &nothing_held())));
    let other_content = canvas_of(vec![im(1, rgba(4, 4, 11))]);
    assert_ne!(a, keys(&decode("pa", &other_content, &nothing_held())));
    let other_size = canvas_of(vec![im(1, rgba(4, 5, 10))]);
    assert_ne!(a, keys(&decode("pa", &other_size, &nothing_held())));
    assert!(a[0].as_ref().unwrap().starts_with("canvas://pa/1/"));
}

#[test]
fn a_placement_carries_its_id_and_rectangle() {
    let mut a = im(7, rgba(2, 2, 1));
    a.x = 3;
    a.y = 4;
    a.cols = 5;
    a.rows = 6;
    let got = decode("pr", &canvas_of(vec![a]), &nothing_held());
    assert_eq!(got.places.len(), 1);
    assert_eq!(got.places[0].id, 7);
    assert_eq!(got.places[0].rect, CellRect::new(3, 4, 5, 6));
}

#[test]
fn file_hash_follows_size_and_modification_not_only_the_path() {
    let dir = crate::test_support::unique_tmp("cvi_filehash");
    std::fs::create_dir_all(&dir).unwrap();
    let f = dir.join("p.png");
    std::fs::write(&f, b"one").unwrap();
    let h1 = content_hash(&ImageData::File(f.to_path_buf()));
    assert_eq!(
        h1,
        content_hash(&ImageData::File(f.to_path_buf())),
        "stable"
    );
    std::fs::write(&f, b"three").unwrap();
    let h2 = content_hash(&ImageData::File(f.to_path_buf()));
    assert_ne!(h1, h2, "the file changed under the same path");
    let g = dir.join("q.png");
    std::fs::write(&g, b"three").unwrap();
    assert_ne!(
        h2,
        content_hash(&ImageData::File(g.to_path_buf())),
        "another path"
    );
}

#[test]
fn file_hash_follows_the_modification_time_when_the_size_is_the_same() {
    let dir = crate::test_support::unique_tmp("cvi_filemtime");
    std::fs::create_dir_all(&dir).unwrap();
    let f = dir.join("p.png");
    std::fs::write(&f, b"aaaa").unwrap();
    let t0 = std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let set = |t: std::time::SystemTime| {
        std::fs::OpenOptions::new()
            .write(true)
            .open(&f)
            .unwrap()
            .set_modified(t)
            .unwrap();
    };
    set(t0);
    let a = content_hash(&ImageData::File(f.to_path_buf()));
    // The same size, rewritten, a moment later.
    std::fs::write(&f, b"bbbb").unwrap();
    set(t0 + Duration::from_secs(10));
    let b = content_hash(&ImageData::File(f.to_path_buf()));
    assert_ne!(
        a, b,
        "a same-size rewrite under the same id is another picture"
    );
    // Only the time moves.
    set(t0 + Duration::from_secs(20));
    assert_ne!(b, content_hash(&ImageData::File(f.to_path_buf())));
    // A file that is not there hashes (to its path alone) without failing.
    let _ = content_hash(&ImageData::File(dir.join("missing.png")));
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

#[test]
fn numbers_in_the_hash_keep_all_their_bits() {
    let mut a = Fnv::new();
    a.num(1);
    let mut b = Fnv::new();
    b.num(1 << 40);
    let mut c = Fnv::new();
    c.num((1 << 40) + 1);
    assert_ne!(a.0, b.0);
    assert_ne!(b.0, c.0);
}

// ---- unchanged pictures ----------------------------------------------------------------------

#[test]
fn an_unchanged_picture_resolves_to_what_the_last_answer_named_for_the_id() {
    let first = decode(
        "pu",
        &canvas_of(vec![im(1, rgba(4, 4, 10))]),
        &nothing_held(),
    );
    let held = held_after(&nothing_held(), &first);
    let want = keys(&first)[0].clone();
    let second = decode("pu", &canvas_of(vec![none(1)]), &held);
    assert_eq!(keys(&second), vec![want], "the same key, no hashing needed");
    assert!(second.fresh.is_empty(), "nothing decoded for it");
    // Moved elsewhere in the same answer: still the one picture.
    let mut moved = none(1);
    moved.x = 9;
    let third = decode("pu", &canvas_of(vec![none(1), moved]), &held);
    assert_eq!(third.places[0].key, third.places[1].key);
    assert_eq!(third.places[1].rect.x, 9);
}

#[test]
fn an_unchanged_picture_of_an_unknown_id_is_a_failed_placement_without_a_panic() {
    let c = canvas_of(vec![none(1), im(2, rgba(4, 4, 10))]);
    let got = decode("pa", &c, &nothing_held());
    assert_eq!(got.places.len(), 2);
    assert!(got.places[0].key.is_none(), "the cells under it stay");
    assert!(got.places[1].key.is_some());
    assert_eq!(got.fresh.len(), 1, "only the real picture is decoded");
    assert!(matches!(&got.fresh[0].1, CanvasDecoded::Still(_)));
    // An id the last answer did not name is unknown too.
    let held = held_after(&nothing_held(), &got);
    let other = decode("pa", &canvas_of(vec![none(9)]), &held);
    assert!(other.places[0].key.is_none());
}

#[test]
fn the_first_placement_of_an_id_decides_its_picture() {
    // One id is one picture: a later placement of the id (a tiled sprite) is the same key.
    let c = canvas_of(vec![im(1, rgba(4, 4, 1)), none(1), im(1, rgba(4, 4, 2))]);
    let got = decode("pt", &c, &nothing_held());
    assert_eq!(got.places[0].key, got.places[1].key);
    assert_eq!(got.places[0].key, got.places[2].key);
    assert_eq!(got.fresh.len(), 1);
}

#[test]
fn a_known_key_is_not_decoded_again() {
    let c = canvas_of(vec![im(1, rgba(4, 4, 77))]);
    let first = decode("pk", &c, &nothing_held());
    assert_eq!(first.fresh.len(), 1);
    let held = held_after(&nothing_held(), &first);
    let again = decode("pk", &c, &held);
    assert_eq!(keys(&again), keys(&first));
    assert!(again.fresh.is_empty(), "nothing decoded for a held picture");
    // A new picture next to a held one: only the new one.
    let c2 = canvas_of(vec![im(1, rgba(4, 4, 77)), im(2, rgba(4, 4, 78))]);
    let both = decode("pk", &c2, &held);
    assert_eq!(both.places.len(), 2);
    assert_eq!(both.fresh.len(), 1);
    assert_eq!(
        both.places[1].key.as_deref(),
        Some(both.fresh[0].0.to_string_lossy().as_ref())
    );
}

#[test]
fn decoded_count_records_only_real_decodes() {
    let data = rgba(3, 3, 123);
    let hash = content_hash(&data);
    let c = canvas_of(vec![im(1, data.clone()), im(1, data)]);
    let before = seen_for_tests::decoded_count(hash);
    let first = decode("pc", &c, &nothing_held());
    assert_eq!(
        seen_for_tests::decoded_count(hash),
        before + 1,
        "twice listed, decoded once"
    );
    let held = held_after(&nothing_held(), &first);
    decode("pc", &c, &held);
    assert_eq!(seen_for_tests::decoded_count(hash), before + 1);
}

#[test]
fn the_same_content_under_two_ids_is_decoded_once_and_held_twice() {
    let data = rgba(5, 5, 201);
    let hash = content_hash(&data);
    let before = seen_for_tests::decoded_count(hash);
    let got = decode(
        "ps",
        &canvas_of(vec![im(1, data.clone()), im(2, data)]),
        &nothing_held(),
    );
    assert_eq!(seen_for_tests::decoded_count(hash), before + 1);
    assert_eq!(got.fresh.len(), 2, "one entry per id");
    assert_ne!(got.fresh[0].0, got.fresh[1].0);
    assert!(matches!(got.fresh[1].1, CanvasDecoded::Still(_)));
}

// ---- limits --------------------------------------------------------------------------------------

#[test]
fn the_limits_are_pinned() {
    assert_eq!(MAX_CANVAS_IMAGES, 256);
    assert_eq!(MAX_CANVAS_BYTES, 128 * 1024 * 1024);
    assert_eq!(MAX_CANVAS_HELD_BYTES, 256 * 1024 * 1024);
    assert_eq!(MAX_DECODES_PER_ANSWER, 32);
    assert_eq!(MAX_INPUT_BYTES_PER_ANSWER, 64 * 1024 * 1024);
    assert_eq!(MIN_FRAME_DELAY, Duration::from_millis(20));
    assert_eq!(
        konoma_plugin::view::CANVAS_MAX_PICTURES,
        MAX_CANVAS_IMAGES,
        "the API's distinct-id limit is the host's"
    );
    let p = CanvasLimits::PRODUCTION;
    assert_eq!(
        (p.held_bytes, p.answer_bytes, p.decodes, p.input_bytes),
        (
            MAX_CANVAS_HELD_BYTES,
            MAX_CANVAS_BYTES,
            MAX_DECODES_PER_ANSWER,
            MAX_INPUT_BYTES_PER_ANSWER
        )
    );
}

#[test]
fn ids_past_the_limit_are_not_looked_at() {
    let images: Vec<CanvasImage> = (0..MAX_CANVAS_IMAGES as u64 + 3)
        .map(|i| im(i, rgba(1, 1, (i % 250) as u8 + 1)))
        .collect();
    let got = decode_within(
        &canvas_of(images),
        &nothing_held(),
        small(|l| l.decodes = 10_000),
    );
    assert_eq!(
        got.places.len(),
        MAX_CANVAS_IMAGES + 3,
        "every placement is listed"
    );
    assert!(got.places[..MAX_CANVAS_IMAGES]
        .iter()
        .all(|p| p.key.is_some()));
    assert!(
        got.places[MAX_CANVAS_IMAGES..]
            .iter()
            .all(|p| p.key.is_none()),
        "the ones past the limit have no picture: their cells stay"
    );
    assert_eq!(got.fresh.len(), MAX_CANVAS_IMAGES);
}

#[test]
fn one_answer_decodes_a_limited_number_and_the_rest_waits_without_failing() {
    let n = MAX_DECODES_PER_ANSWER as u64 * 2 + 5;
    let images: Vec<CanvasImage> = (0..n)
        .map(|i| im(i, rgba(1, 1, (i % 250) as u8 + 1)))
        .collect();
    let c = canvas_of(images);
    let first = decode("pn", &c, &nothing_held());
    assert_eq!(first.fresh.len(), MAX_DECODES_PER_ANSWER);
    assert_eq!(first.waiting, n as usize - MAX_DECODES_PER_ANSWER);
    assert!(first.more, "another answer is worth asking for");
    assert!(
        first
            .fresh
            .iter()
            .all(|(_, d)| matches!(d, CanvasDecoded::Still(_))),
        "none of the waiting ones is marked failed"
    );
    assert_eq!(first.places.len(), n as usize);
    // The following answers finish the job, and the last one asks for nothing.
    let mut held = held_after(&nothing_held(), &first);
    let mut rounds = 1;
    loop {
        let next = decode("pn", &c, &held);
        rounds += 1;
        held = held_after(&held, &next);
        if !next.more {
            assert_eq!(next.waiting, 0);
            break;
        }
        assert!(rounds < 10);
    }
    assert_eq!(rounds, 3, "ceil(69 / 32)");
    assert_eq!(held.keys.len(), n as usize);
}

#[test]
fn the_input_budget_of_one_answer_stops_further_decodes_but_the_first_always_runs() {
    // Three 600-byte garbage pictures against a 1000-byte budget: the first runs (even if it alone
    // were over), the second starts (600 < 1000), the third waits (1200 >= 1000).
    let junk = |n: u8| ImageData::Encoded(Arc::from(vec![n; 600]));
    let c = canvas_of(vec![im(1, junk(1)), im(2, junk(2)), im(3, junk(3))]);
    let got = decode_within(&c, &nothing_held(), small(|l| l.input_bytes = 1000));
    assert_eq!(got.fresh.len(), 2);
    assert_eq!(got.waiting, 1);
    assert!(got.more);
    // A single picture over the budget still runs.
    let one = decode_within(
        &canvas_of(vec![im(1, junk(1))]),
        &nothing_held(),
        small(|l| l.input_bytes = 10),
    );
    assert_eq!(one.fresh.len(), 1);
    assert_eq!(one.waiting, 0);
    assert!(!one.more);
}

#[test]
fn the_pixel_budget_of_one_answer_stops_further_decodes() {
    // 100 x 100 x 4 = 40,000 bytes each; the budget of an answer is 70,000: two run, the third
    // waits (it is not failed and it is not refused for good).
    let c = canvas_of(vec![
        im(1, rgba(100, 100, 1)),
        im(2, rgba(100, 100, 2)),
        im(3, rgba(100, 100, 3)),
    ]);
    let got = decode_within(&c, &nothing_held(), small(|l| l.answer_bytes = 70_000));
    assert_eq!(got.fresh.len(), 2);
    assert_eq!(got.waiting, 1);
    assert!(got.more);
    // Exactly at the budget is over it.
    let exact = decode_within(&c, &nothing_held(), small(|l| l.answer_bytes = 40_000));
    assert_eq!(exact.fresh.len(), 1, "the first fills it");
    let one_more = decode_within(&c, &nothing_held(), small(|l| l.answer_bytes = 40_001));
    assert_eq!(one_more.fresh.len(), 2);
}

#[test]
fn the_memory_bound_counts_what_the_canvas_keeps_and_refuses_the_rest_for_now() {
    // The canvas keeps picture 9 (200,000 bytes held) and adds two 40,000-byte pictures into a
    // 250,000-byte bound: the first fits (240,000), the second does not.
    let mut held = nothing_held();
    let kept: Arc<str> = Arc::from("canvas://pw/9/00000000000000aa");
    held.keys.insert(PathBuf::from(&*kept), 200_000);
    held.by_id.insert(9, kept.clone());
    let c = canvas_of(vec![
        none(9),
        im(1, rgba(100, 100, 1)),
        im(2, rgba(100, 100, 2)),
    ]);
    let l = small(|l| l.held_bytes = 250_000);
    let got = decode_within(&c, &held, l);
    assert_eq!(got.fresh.len(), 1, "one fits");
    assert_eq!(got.refused, 1);
    assert_eq!(got.waiting, 0);
    assert!(
        !got.more,
        "asking again does not help; the picture is not failed"
    );
    assert!(
        !matches!(got.fresh[0].1, CanvasDecoded::Failed(_)),
        "a refusal is not a failure"
    );
    // Both are named by the canvas, so both have a key (the refused one has no entry yet).
    assert!(got.places[1].key.is_some() && got.places[2].key.is_some());
    // What the canvas does not name any more does not count against the bound.
    let mut stale = nothing_held();
    stale
        .keys
        .insert(PathBuf::from("canvas://pw/9/00000000000000aa"), 10_000_000);
    let freed = decode_within(
        &canvas_of(vec![im(1, rgba(100, 100, 1)), im(2, rgba(100, 100, 2))]),
        &stale,
        l,
    );
    assert_eq!(freed.fresh.len(), 2);
    assert_eq!(freed.refused, 0);
}

#[test]
fn the_memory_bound_counts_the_entry_an_id_is_still_drawn_from() {
    // Id 1 is drawn from an old 200,000-byte entry while its new content comes: both are held
    // until the new one can be drawn.
    let mut held = nothing_held();
    let old: Arc<str> = Arc::from("canvas://pw/1/00000000000000aa");
    held.keys.insert(PathBuf::from(&*old), 200_000);
    held.front.insert(1, old);
    let c = canvas_of(vec![im(1, rgba(100, 100, 1))]);
    let tight = decode_within(&c, &held, small(|l| l.held_bytes = 230_000));
    assert!(tight.fresh.is_empty(), "200,000 + 40,000 > 230,000");
    assert_eq!(tight.refused, 1);
    let roomy = decode_within(&c, &held, small(|l| l.held_bytes = 240_000));
    assert_eq!(roomy.fresh.len(), 1, "exactly the bound fits");
}

#[test]
fn a_picture_that_can_never_fit_fails_for_good() {
    let c = canvas_of(vec![im(1, rgba(100, 100, 1))]);
    let got = decode_within(&c, &nothing_held(), small(|l| l.held_bytes = 39_999));
    assert!(matches!(
        got.fresh[0].1,
        CanvasDecoded::Failed(crate::preview::image::ImageFailure::TooLarge)
    ));
}

#[test]
fn a_copy_for_a_second_id_is_refused_when_it_does_not_fit() {
    let data = rgba(100, 100, 9);
    let c = canvas_of(vec![im(1, data.clone()), im(2, data)]);
    let got = decode_within(&c, &nothing_held(), small(|l| l.held_bytes = 60_000));
    assert_eq!(got.fresh.len(), 1, "the copy would be held twice");
    assert_eq!(got.refused, 1);
}

// ---- animations -----------------------------------------------------------------------------------

#[test]
fn frames_decode_as_an_animation_and_a_bad_frame_fails_the_whole_picture() {
    let ok = ImageData::Frames(vec![
        (rgba(2, 2, 1), Duration::from_millis(30)),
        (rgba(2, 2, 2), Duration::from_millis(40)),
    ]);
    let got = decode("pf", &canvas_of(vec![im(1, ok)]), &nothing_held());
    match &got.fresh[0].1 {
        CanvasDecoded::Frames(f) => {
            assert_eq!(f.len(), 2);
            assert_eq!(f[1].1, Duration::from_millis(40), "delays kept");
        }
        _ => panic!("an animation"),
    }
    let bad = ImageData::Frames(vec![
        (rgba(2, 2, 1), Duration::from_millis(30)),
        (
            ImageData::Encoded(Arc::from(&b"nope"[..])),
            Duration::from_millis(30),
        ),
    ]);
    let got = decode("pf", &canvas_of(vec![im(1, bad)]), &nothing_held());
    assert!(matches!(got.fresh[0].1, CanvasDecoded::Failed(_)));
    // A frame list inside a frame list is not an animation of animations.
    let nested = ImageData::Frames(vec![(
        ImageData::Frames(vec![(rgba(1, 1, 1), Duration::ZERO)]),
        Duration::ZERO,
    )]);
    let got = decode("pf", &canvas_of(vec![im(1, nested)]), &nothing_held());
    assert!(matches!(got.fresh[0].1, CanvasDecoded::Failed(_)));
    // An empty list is nothing to show.
    let got = decode(
        "pf",
        &canvas_of(vec![im(1, ImageData::Frames(vec![]))]),
        &nothing_held(),
    );
    assert!(matches!(got.fresh[0].1, CanvasDecoded::Failed(_)));
}

#[test]
fn a_frame_is_shown_for_at_least_twenty_milliseconds() {
    let ms = Duration::from_millis;
    let frames = ImageData::Frames(vec![
        (rgba(1, 1, 1), Duration::ZERO),
        (rgba(1, 1, 2), ms(5)),
        (rgba(1, 1, 3), ms(19)),
        (rgba(1, 1, 4), ms(20)),
        (rgba(1, 1, 5), ms(50)),
    ]);
    let got = decode("pd", &canvas_of(vec![im(1, frames)]), &nothing_held());
    let CanvasDecoded::Frames(f) = &got.fresh[0].1 else {
        panic!("an animation")
    };
    let delays: Vec<_> = f.iter().map(|(_, d)| *d).collect();
    assert_eq!(delays, vec![ms(20), ms(20), ms(20), ms(20), ms(50)]);
}

#[test]
fn the_decoded_frames_of_an_animation_are_bounded_by_the_inline_gif_budget() {
    // 2048 x 2048 x 4 = 16 MiB a frame; the budget is 32 MiB (`MAX_GIF_BYTES_INLINE`).
    let frame = |v: u8| rgba(2048, 2048, v);
    let d = Duration::from_millis(50);
    let two = ImageData::Frames(vec![(frame(1), d), (frame(2), d)]);
    let got = decode("pgb", &canvas_of(vec![im(1, two)]), &nothing_held());
    assert!(
        matches!(&got.fresh[0].1, CanvasDecoded::Frames(f) if f.len() == 2),
        "exactly the budget fits"
    );
    let three = ImageData::Frames(vec![(frame(1), d), (frame(2), d), (frame(3), d)]);
    let got = decode("pgb", &canvas_of(vec![im(1, three)]), &nothing_held());
    assert!(
        matches!(
            got.fresh[0].1,
            CanvasDecoded::Failed(crate::preview::image::ImageFailure::TooLarge)
        ),
        "the frames add up"
    );
}

#[test]
fn every_frame_counts_in_the_bytes_of_an_animation() {
    let one = |v: u8| {
        (
            image::DynamicImage::new_rgba8(10, 10),
            Duration::from_millis(v as u64),
        )
    };
    assert_eq!(
        decoded_bytes(&CanvasDecoded::Still(image::DynamicImage::new_rgba8(
            10, 10
        ))),
        400
    );
    assert_eq!(
        decoded_bytes(&CanvasDecoded::Frames(vec![one(1), one(2), one(3)])),
        1200
    );
    assert_eq!(
        decoded_bytes(&CanvasDecoded::Failed(
            crate::preview::image::ImageFailure::Corrupt
        )),
        0
    );
}

#[test]
fn an_encoded_gif_with_several_frames_is_an_animation_and_a_png_is_a_still() {
    use image::codecs::gif::GifEncoder;
    use image::{Delay, Frame, Rgba, RgbaImage};
    let mut gif = Vec::new();
    {
        let mut enc = GifEncoder::new(&mut gif);
        for c in [[200u8, 0, 0], [0, 200, 0]] {
            enc.encode_frame(Frame::from_parts(
                RgbaImage::from_pixel(8, 8, Rgba([c[0], c[1], c[2], 255])),
                0,
                0,
                Delay::from_numer_denom_ms(50, 1),
            ))
            .unwrap();
        }
    }
    let got = decode(
        "pg",
        &canvas_of(vec![im(1, ImageData::Encoded(gif.into()))]),
        &nothing_held(),
    );
    assert!(matches!(&got.fresh[0].1, CanvasDecoded::Frames(f) if f.len() == 2));
    let mut png = Vec::new();
    {
        use image::ImageEncoder;
        image::codecs::png::PngEncoder::new(&mut png)
            .write_image(&[1, 2, 3, 255], 1, 1, image::ExtendedColorType::Rgba8)
            .unwrap();
    }
    let got = decode(
        "pg",
        &canvas_of(vec![im(1, ImageData::Encoded(png.into()))]),
        &nothing_held(),
    );
    assert!(matches!(got.fresh[0].1, CanvasDecoded::Still(_)));
}

// ---- cancellation and files --------------------------------------------------------------------

#[test]
fn a_cancelled_job_decodes_nothing() {
    let cancel = crate::preview::office::Cancel::new(|| true);
    let c = canvas_of(vec![im(1, rgba(9, 9, 9))]);
    let got = decode_canvas(
        "pcx",
        &c,
        &nothing_held(),
        Path::new("/x"),
        800,
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
    let got = decode_canvas(
        "pcf",
        &canvas_of(vec![im(1, frames)]),
        &nothing_held(),
        Path::new("/x"),
        800,
        Some(&cancel),
        false,
    );
    assert!(matches!(
        got.fresh[0].1,
        CanvasDecoded::Failed(crate::preview::image::ImageFailure::Cancelled)
    ));
}

#[test]
fn only_a_regular_file_is_opened_for_a_plugin() {
    let dir = crate::test_support::unique_tmp("cvi_regular");
    std::fs::create_dir_all(&dir).unwrap();
    // A directory.
    let got = decode(
        "pfile",
        &canvas_of(vec![im(1, ImageData::File(dir.to_path_buf()))]),
        &nothing_held(),
    );
    assert!(matches!(got.fresh[0].1, CanvasDecoded::Failed(_)));
    // A FIFO: opening it would block the worker for good, so it must not be opened. The decode
    // runs on a thread so that a regression fails here instead of hanging the suite.
    let fifo = dir.join("pipe.png");
    let made = std::process::Command::new("mkfifo").arg(&fifo).status();
    if made.is_ok_and(|s| s.success()) {
        let (tx, rx) = std::sync::mpsc::channel();
        let path = fifo.to_path_buf();
        std::thread::spawn(move || {
            let got = decode(
                "pfile",
                &canvas_of(vec![im(1, ImageData::File(path.clone()))]),
                &nothing_held(),
            );
            let gif = canvas_of(vec![im(
                2,
                ImageData::Frames(vec![(ImageData::File(path), Duration::ZERO)]),
            )]);
            let got2 = decode("pfile", &gif, &nothing_held());
            let _ = tx.send((
                matches!(got.fresh[0].1, CanvasDecoded::Failed(_)),
                matches!(got2.fresh[0].1, CanvasDecoded::Failed(_)),
            ));
        });
        let (plain, inside_frames) = rx
            .recv_timeout(Duration::from_secs(10))
            .expect("a FIFO was opened (the decode is stuck)");
        assert!(plain && inside_frames, "refused as a file, and as a frame");
    }
    // A regular file still works.
    let png = dir.join("ok.png");
    image::RgbaImage::from_pixel(3, 3, image::Rgba([1, 2, 3, 255]))
        .save(&png)
        .unwrap();
    let got = decode(
        "pfile",
        &canvas_of(vec![im(1, ImageData::File(png.to_path_buf()))]),
        &nothing_held(),
    );
    assert!(matches!(got.fresh[0].1, CanvasDecoded::Still(_)));
}

// ---- landing ---------------------------------------------------------------------------------------

#[test]
fn a_cancelled_picture_leaves_no_entry_behind() {
    let (mut app, _dir) = new_app("cvi_cancelled");
    let key = PathBuf::from("canvas://p/1/00000000000000aa");
    app.canvas_pictures_landed(CanvasPictures {
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
        !app.md_image_cache.contains_key(&key),
        "asked for again next time"
    );
}

#[test]
fn landing_inserts_prunes_and_forgets() {
    let (mut app, _dir) = new_app("cvi_landing");
    let c1 = canvas_of(vec![im(1, rgba(4, 4, 1)), im(2, rgba(4, 4, 2))]);
    let p1 = decode("pl", &c1, &nothing_held());
    let keys1 = keys(&p1);
    app.canvas_pictures_landed(p1);
    assert_eq!(app.md_image_cache.len(), 2);
    let placed: Vec<_> = app
        .canvas_places
        .iter()
        .map(|p| p.key.as_deref().map(str::to_string))
        .collect();
    assert_eq!(placed, keys1);
    // An unrelated (Markdown / media-diff) entry is not touched by a canvas landing.
    app.md_image_cache
        .insert(PathBuf::from("media-diff://o/00/1"), Default::default());
    // The next answer keeps picture 1 and replaces picture 2's content. Nothing has been drawn,
    // so no entry is "drawn from": the replaced content goes.
    let c2 = canvas_of(vec![im(1, rgba(4, 4, 1)), im(2, rgba(4, 4, 3))]);
    let held = {
        let mut h = nothing_held();
        h.keys = app
            .md_image_cache
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
        app.md_image_cache
            .contains_key(Path::new(k.as_ref().unwrap()))
    };
    assert!(has(&keys1[0]));
    assert!(!has(&keys1[1]), "the replaced one is dropped");
    assert!(has(&keys2[1]));
    assert!(app
        .md_image_cache
        .contains_key(&PathBuf::from("media-diff://o/00/1")));
    // A canvas without pictures drops them all.
    app.canvas_pictures_landed(decode("pl", &canvas_of(vec![]), &nothing_held()));
    assert!(app
        .md_image_cache
        .keys()
        .all(|k| !crate::preview::markdown::is_canvas_url(&k.to_string_lossy())));
    assert!(app.canvas_places.is_empty());
    // And forgetting clears whatever is left.
    app.canvas_pictures_landed(decode("pl", &c1, &nothing_held()));
    app.forget_canvas_pictures();
    assert!(app.canvas_places.is_empty());
    assert!(app
        .md_image_cache
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
        app.md_image_cache.contains_key(Path::new(&*old)),
        "still drawn from"
    );
    assert!(app.md_image_cache.contains_key(Path::new(&*new)));
    assert_eq!(app.md_image_cache.len(), 2);
    // The new content failed for good: the old picture ends (the cells show). The frame only
    // moved the pointer: the entry itself goes when the next answer lands (the prune), never in a
    // frame.
    app.md_image_cache.get_mut(Path::new(&*new)).unwrap().failed = true;
    let shown = app.canvas_shown_key(1, &new, 2, 2, 2);
    assert_eq!(&*shown, &*new);
    assert!(!app.canvas_front.contains_key(&1));
    assert!(
        app.md_image_cache.contains_key(Path::new(&*old)),
        "a frame removes no entry"
    );
    let next = decode(
        "pfr",
        &canvas_of(vec![im(1, rgba(4, 4, 2))]),
        &app.canvas_known_keys_for_test(),
    );
    app.canvas_pictures_landed(next);
    assert!(!app.md_image_cache.contains_key(Path::new(&*old)));
    assert!(app.md_image_cache.contains_key(Path::new(&*new)));
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
    app.md_image_cache.remove(Path::new(&*old));
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
    assert!(app.md_image_cache.contains_key(Path::new(&*k1)));
    assert!(
        !app.md_image_cache.contains_key(Path::new(&*k2)),
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
    assert!(app.md_image_cache.contains_key(Path::new(&*k2)));
    assert_eq!(app.md_image_cache.len(), 1);
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
        app.md_image_cache[&key].decoded.is_some(),
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

#[test]
fn the_apis_rgba_side_limit_is_the_decode_cap_of_the_host() {
    assert_eq!(konoma_plugin::view::CANVAS_MAX_RGBA_SIDE, MD_IMAGE_MAX_SIDE);
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
    app.use_kitty = true;
    let c = canvas_of(vec![im(1, rgba(4, 4, 1)), im(2, rgba(4, 4, 2))]);
    app.canvas_pictures_landed(decode("pk", &c, &nothing_held()));
    // The encodes pinned ids for both slots (and Markdown has its own, which stay).
    let slot1 = PathBuf::from("canvas://pk/1");
    let slot2 = PathBuf::from("canvas://pk/2");
    app.md_kitty_ids
        .insert(slot1.clone(), [vec![11, 12], vec![13], vec![]]);
    app.md_kitty_ids
        .insert(slot2.clone(), [vec![21], vec![], vec![]]);
    app.md_kitty_ids
        .insert(PathBuf::from("/doc/a.png"), [vec![99], vec![], vec![]]);
    // Picture 1's content changes and picture 2 leaves.
    let c2 = canvas_of(vec![im(1, rgba(4, 4, 7))]);
    app.canvas_pictures_landed(decode("pk", &c2, &nothing_held()));
    assert!(
        app.md_kitty_ids.contains_key(&slot1),
        "same picture: ids kept"
    );
    assert!(!app.md_kitty_ids.contains_key(&slot2));
    assert!(app.md_kitty_ids.contains_key(&PathBuf::from("/doc/a.png")));
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
    assert!(app.md_kitty_ids.contains_key(&PathBuf::from("/doc/a.png")));
}

#[test]
fn deletes_go_only_to_a_kitty_terminal_and_follow_the_tmux_wrapping() {
    let (mut app, _dir) = new_app("cvi_deletes");
    app.kitty_deletes = vec![5];
    app.use_kitty = false;
    assert!(
        app.take_kitty_deletes().is_empty(),
        "no kitty, nothing is written"
    );
    assert!(
        app.kitty_deletes.is_empty(),
        "and the queue does not pile up"
    );
    app.use_kitty = true;
    app.kitty_deletes = vec![5, 6];
    app.kitty_is_tmux = true;
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
    assert_eq!(app.md_image_cache.len(), MAX_DECODES_PER_ANSWER);
    assert_eq!(held.kept.len(), 8, "the host kept the 8 it deferred");
    // The plugin does what the API tells it: pixels once, then `unchanged` for all.
    let unchanged = canvas_of((0..n).map(none).collect());
    let mut held = held;
    for _ in 0..3 {
        held = land(&mut app, decode("pk", &unchanged, &held));
    }
    assert_eq!(app.md_image_cache.len(), n as usize, "all arrived");
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
    assert_eq!(app.md_image_cache.len(), 1);
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
    assert!(app.md_image_cache.contains_key(Path::new(&*k2)));
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
    assert_eq!(app.md_image_cache.len(), 2);
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
    app.md_image_cache.get_mut(Path::new(&*kb)).unwrap().failed = true;
    app.canvas_shown_key(1, &kb, 2, 2, 2);
    assert!(
        app.md_image_cache.contains_key(Path::new(&*ka)),
        "no frame drops an entry"
    );
    // The answer of the job names A again.
    let ga2 = decode("pfl", &a, &snapshot);
    assert!(ga2.fresh.is_empty(), "A is held, so nothing is decoded");
    app.canvas_pictures_landed(ga2);
    assert!(
        app.md_image_cache.contains_key(Path::new(&*ka)),
        "the placement names an entry that is there"
    );
}

// ---- batch F: hashing --------------------------------------------------------------------------------

#[test]
fn a_buffer_cloned_into_many_frames_is_hashed_once_and_the_hash_does_not_change() {
    let shared = Arc::new(vec![7u8; 100_000]);
    let frame = |p: &Arc<Vec<u8>>| ImageData::Rgba {
        width: 100,
        height: 250,
        pixels: p.clone(),
    };
    let d = Duration::from_millis(40);
    let same = ImageData::Frames((0..50).map(|_| (frame(&shared), d)).collect());
    let apart = ImageData::Frames(
        (0..50)
            .map(|_| (frame(&Arc::new(vec![7u8; 100_000])), d))
            .collect(),
    );
    let mut h = Hasher::new(None);
    let hs = h.content(&same).unwrap();
    assert_eq!(h.bytes, 100_000, "one buffer, hashed once");
    let mut h2 = Hasher::new(None);
    let ha = h2.content(&apart).unwrap();
    assert_eq!(h2.bytes, 50 * 100_000, "distinct buffers are each hashed");
    assert_eq!(hs, ha, "the identity of a buffer is not part of the hash");
}

#[test]
fn hashing_looks_at_the_cancellation_while_a_large_buffer_is_read() {
    let big = ImageData::Encoded(Arc::from(vec![1u8; HASH_CHUNK * 5 + 1]));
    let questions = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let q = questions.clone();
    let cancel = crate::preview::office::Cancel::new(move || {
        q.fetch_add(1, std::sync::atomic::Ordering::SeqCst) >= 2
    });
    let mut h = Hasher::new(Some(&cancel));
    assert_eq!(
        h.content(&big),
        None,
        "cancelled in the middle of the buffer"
    );
    assert_eq!(h.bytes, 0, "and nothing is counted as hashed");
    assert!(questions.load(std::sync::atomic::Ordering::SeqCst) <= 4);
    // A cancelled job answers with nothing named and nothing to decode.
    let got = decode_canvas(
        "phc",
        &canvas_of(vec![im(1, big)]),
        &nothing_held(),
        Path::new("/x"),
        800,
        Some(&crate::preview::office::Cancel::new(|| true)),
        false,
    );
    assert!(got.fresh.is_empty() && got.sources.is_empty());
    assert!(got.places[0].key.is_none());
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
    assert_eq!(app.md_image_cache.len(), 3);
    assert!(app.canvas_kept.is_empty());
}

#[test]
fn an_unhashed_picture_keeps_showing_the_previous_content_of_its_id() {
    // Id 3 had content before; its new content is not hashed this time: the placement still names
    // the old key (it is the same picture until the new one comes), not a failed placement.
    let l = small(|l| l.input_bytes = 10);
    let mut held = nothing_held();
    let old: Arc<str> = Arc::from("canvas://pw/3/00000000000000aa");
    held.keys.insert(PathBuf::from(&*old), 40_000);
    held.by_id.insert(3, old.clone());
    let c = canvas_of(vec![im(1, rgba(100, 100, 24)), im(3, rgba(100, 100, 25))]);
    let got = decode_within(&c, &held, l);
    assert_eq!(got.places[1].key.as_deref(), Some(&*old));
    assert_eq!(got.waiting, 1);
}

#[test]
fn the_same_buffer_sent_again_reuses_the_hash_of_the_kept_picture() {
    let data = rgba(100, 100, 26);
    assert!(same_source(&data, &data.clone()));
    assert!(
        !same_source(&data, &rgba(100, 100, 26)),
        "equal bytes, another buffer"
    );
    assert!(
        !same_source(
            &ImageData::File(PathBuf::from("/a")),
            &ImageData::File(PathBuf::from("/a"))
        ),
        "a file may have changed"
    );
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
