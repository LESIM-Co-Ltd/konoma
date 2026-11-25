//! The landing of a canvas answer into an inline-image cache, the per-picture state of the host
//! (front pointers, kept sources, placements) and the kitty ids: the tests that need no App.
//! `TestApp` is the two things a host keeps (the cache and the canvas host) with the names the
//! host's own tests use, so these tests read as they did where they began.

use konoma_widgets::inline_images::InlineImageCache;

use super::*;

struct TestApp {
    canvas: CanvasPictureHost,
    md_images: InlineImageCache,
}

/// The cache budget the tests land under: far above anything they file.
const BUDGET: u64 = u64::MAX;

fn new_app(_name: &str) -> (TestApp, ()) {
    (
        TestApp {
            canvas: CanvasPictureHost::default(),
            md_images: InlineImageCache::default(),
        },
        (),
    )
}

impl TestApp {
    fn canvas_pictures_landed(&mut self, pics: CanvasPictures) -> bool {
        self.canvas
            .landed(&mut self.md_images, pics, BUDGET)
            .ask_again
    }

    fn canvas_pictures_landed_within(&mut self, pics: CanvasPictures, held_bytes: u64) -> bool {
        self.canvas
            .landed_within(&mut self.md_images, pics, BUDGET, held_bytes)
            .ask_again
    }

    fn canvas_pictures_landed_limits(
        &mut self,
        pics: CanvasPictures,
        held_bytes: u64,
        kept_source_bytes: u64,
    ) -> bool {
        self.canvas
            .landed_limits(
                &mut self.md_images,
                pics,
                BUDGET,
                held_bytes,
                kept_source_bytes,
            )
            .ask_again
    }

    fn canvas_known_keys_for_test(&self) -> CanvasHeld {
        self.canvas.known_keys(&self.md_images)
    }

    fn canvas_held_bytes(&self) -> u64 {
        CanvasPictureHost::held_bytes(&self.md_images)
    }

    fn canvas_shown_key(
        &mut self,
        id: u64,
        wanted: &Arc<str>,
        cols: u16,
        rows: u16,
        vis_rows: u16,
    ) -> Arc<str> {
        self.canvas
            .shown_key(&self.md_images, id, wanted, cols, rows, vis_rows)
    }

    fn forget_canvas_pictures(&mut self) {
        self.canvas.forget(&mut self.md_images);
    }

    fn evict_md_images_to(&mut self, budget: u64) {
        self.md_images.evict_to(budget);
    }

    fn begin_md_image_frame(&mut self) {
        self.md_images.begin_pass();
    }
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
    app.canvas.front.insert(1, old.clone());
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
    assert!(!app.canvas.front.contains_key(&1));
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
    app.canvas.front.insert(1, old.clone());
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
fn canvas_entries_are_never_evicted_by_the_cache_budget() {
    // Their pixels cannot be rebuilt (the bytes were the plugin's), so the budget must not drop them.
    let (mut app, _dir) = new_app("cvi_noevict");
    app.canvas_pictures_landed(decode(
        "pe",
        &canvas_of(vec![im(1, rgba(64, 64, 9))]),
        &nothing_held(),
    ));
    let key = PathBuf::from(app.canvas.places[0].key.as_deref().unwrap());
    app.begin_md_image_frame();
    app.begin_md_image_frame();
    app.evict_md_images_to(0);
    assert!(
        app.md_images.cache[&key].decoded.is_some(),
        "the pixels are still there"
    );
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
    assert_eq!(app.canvas.kept.len(), 1);
    assert_eq!(app.canvas.kept[&2].needs, Some(40_000));
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
    assert!(app.canvas.kept.is_empty());
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
    assert_eq!(app.canvas.kept.len(), 1);
    assert_eq!(app.canvas.kept[&2].key.as_deref(), Some(&*k2));
    assert_eq!(app.canvas.kept[&2].needs, Some(40_000));
    // With room (the bound raised in the test), the next answer, unchanged, brings it in.
    let held = app.canvas_known_keys_for_test();
    let next = decode("pkl", &canvas_of(vec![none(1), none(2)]), &held);
    assert_eq!(next.fresh.len(), 1);
    app.canvas_pictures_landed(next);
    assert!(app.md_images.cache.contains_key(Path::new(&*k2)));
    assert!(app.canvas.kept.is_empty());
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
    assert_eq!(app.canvas.kept.len(), 1);
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
    assert!(app.canvas.kept.is_empty());
    assert_eq!(app.md_images.cache.len(), 2);
}

#[test]
fn what_is_kept_goes_when_the_id_leaves_the_canvas_or_the_show_is_dropped() {
    let (mut app, _dir) = new_app("cvi_kept_gone");
    let l = small(|l| l.decodes = 1);
    let two = canvas_of(vec![im(1, rgba(1, 1, 238)), im(2, rgba(1, 1, 239))]);
    app.canvas_pictures_landed(decode_within(&two, &nothing_held(), l));
    assert!(app.canvas.kept.contains_key(&2));
    // The next canvas does not name id 2.
    let held = app.canvas_known_keys_for_test();
    app.canvas_pictures_landed(decode_within(&canvas_of(vec![none(1)]), &held, l));
    assert!(app.canvas.kept.is_empty(), "released with the id");
    // And with the show.
    app.canvas_pictures_landed(decode_within(&two, &nothing_held(), l));
    app.forget_canvas_pictures();
    assert!(app.canvas.kept.is_empty());
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
    app.canvas.front.insert(1, ka.clone());
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
    assert!(app.canvas.kept.contains_key(&3));
    // Unchanged: the kept one goes first, is hashed and decoded.
    let held = app.canvas_known_keys_for_test();
    let next = decode_within(&canvas_of(vec![none(1), none(2), none(3)]), &held, l);
    assert_eq!(next.fresh.len(), 1, "only the third");
    assert!(next.places[2].key.is_some());
    app.canvas_pictures_landed(next);
    assert_eq!(app.md_images.cache.len(), 3);
    assert!(app.canvas.kept.is_empty());
}

// ---- batch F: the bytes counted ----------------------------------------------------------------------

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
    assert!(app.canvas.kept.is_empty());
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
    let mut ids: Vec<u64> = app.canvas.kept.keys().copied().collect();
    ids.sort();
    assert_eq!(ids, [2, 3], "id 1 was the first deferred, and goes");
    // Two new ones are deferred behind them: now it is the older kept (id 3) that goes.
    let got = decode_within(
        &canvas_of(vec![none(2), none(3), pic(4), pic(5)]),
        &app.canvas_known_keys_for_test(),
        l,
    );
    app.canvas_pictures_landed_limits(got, MAX_CANVAS_HELD_BYTES, cap);
    let mut ids: Vec<u64> = app.canvas.kept.keys().copied().collect();
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
    app.canvas.kept.insert(2, kp(1));
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
    let mut ids: Vec<u64> = app.canvas.kept.keys().copied().collect();
    ids.sort();
    assert_eq!(ids, [5, 6], "the one kept longest (id 2) is dropped");
}

// ---- round 3 of the mutation audit: survivors of the pictures' worker and landing --------------------

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
    app.canvas.front.insert(1, old_key.clone());
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
    assert_eq!(app.canvas.front.get(&1), Some(&old_key));
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
    let mut ids: Vec<u64> = app.canvas.kept.keys().copied().collect();
    ids.sort();
    assert_eq!(ids, [1, 2], "80,000 of 80,000 is within the bound");
}

// ---- the kept sources are shared, not copied; frames are decodes (stage 2.5 / part 2) ------------

#[test]
fn landing_frees_the_replaced_kept_table_off_the_ui_thread() {
    let (mut app, _dir) = new_app("cvi_discard");
    for id in 1..=3u64 {
        app.canvas.kept.insert(
            id,
            KeptPicture {
                key: None,
                hash: None,
                data: Arc::new(animation(10, id as u8)),
                needs: None,
            },
        );
    }
    konoma_base::discard::clear_discarded();
    app.canvas_pictures_landed(decode("pd", &canvas_of(vec![]), &nothing_held()));
    assert!(app.canvas.kept.is_empty());
    let discarded = konoma_base::discard::discarded_types();
    assert!(
        discarded.iter().any(|t| t.contains("KeptPicture")),
        "the old table was handed to the reaper, not dropped here: {discarded:?}"
    );
}

/// Land `got` the way a host does and return what the next job would be told.
fn land(app: &mut TestApp, got: CanvasPictures) -> CanvasHeld {
    app.canvas_pictures_landed(got);
    app.canvas_known_keys_for_test()
}

/// The key of every placement, unwrapped.
fn key_list(p: &CanvasPictures) -> Vec<Arc<str>> {
    p.places.iter().map(|p| p.key.clone().unwrap()).collect()
}

/// Mark the placements as drawn (what the frame does once a picture has a protocol): the id is
/// drawn from its entry. After the landing that put the entries in the cache.
fn draw_places(app: &mut TestApp, places: &[CanvasPlace]) {
    for pl in places {
        app.canvas.front.insert(pl.id, pl.key.clone().unwrap());
    }
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
    assert!(app.canvas.kept.is_empty(), "nothing is kept any more");
    assert!(app.canvas.places.iter().all(|p| p.key.is_some()));
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
    assert!(app.canvas.kept.is_empty());
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
        !app.canvas.front.contains_key(&0),
        "the old picture of id 0 is not shown in place of the new one"
    );
    assert!(
        !app.md_images.cache.contains_key(Path::new(&*old[0])),
        "and its pixels are freed"
    );
    assert_eq!(app.canvas.kept[&0].needs, Some(79_524));
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
    assert!(app.canvas.kept.is_empty());
    assert_eq!(app.canvas_held_bytes(), 79_524 + 80_000);
}

fn answer_of(key: &str, decoded: CanvasDecoded) -> CanvasPictures {
    CanvasPictures {
        scope_fp: 0,
        places: vec![CanvasPlace {
            id: 1,
            rect: CellRect::new(0, 0, 1, 1),
            key: Some(Arc::from(key)),
        }],
        fresh: vec![(PathBuf::from(key), decoded)],
        waiting: 0,
        refused: 0,
        more: false,
        sources: Vec::new(),
    }
}

#[test]
fn landing_reports_a_failed_picture_for_the_host_to_relayout_and_nothing_else() {
    let key = "canvas://p/1/00000000000000cd";
    let mut cache = InlineImageCache::default();
    let mut host = CanvasPictureHost::default();
    // A decoded picture is filed as pixels: nothing to relayout.
    let ok = host.landed(
        &mut cache,
        answer_of(
            key,
            CanvasDecoded::Still(image::DynamicImage::new_rgba8(2, 1 + 1)),
        ),
        BUDGET,
    );
    assert!(!ok.redecorate);
    assert!(cache.cache[Path::new(key)].decoded.is_some());
    // One that could not be decoded is failed for good, and the host is told.
    let other = "canvas://p/1/00000000000000ce";
    let bad = host.landed(
        &mut cache,
        answer_of(
            other,
            CanvasDecoded::Failed(ImageFailure::UnsupportedFormat),
        ),
        BUDGET,
    );
    assert!(bad.redecorate);
    assert!(cache.cache[Path::new(other)].failed);
    assert!(!bad.ask_again);
}

#[test]
fn landing_a_cancelled_picture_leaves_no_entry_and_no_relayout() {
    let key = "canvas://p/1/00000000000000cf";
    let mut cache = InlineImageCache::default();
    let mut host = CanvasPictureHost::default();
    let got = host.landed(
        &mut cache,
        answer_of(key, CanvasDecoded::Failed(ImageFailure::Cancelled)),
        BUDGET,
    );
    assert!(!got.redecorate && !got.ask_again);
    assert!(!cache.cache.contains_key(Path::new(key)));
}

#[test]
fn landing_numbers_each_filed_picture_from_the_caches_counter() {
    // The requests the landing makes come from the cache's counter, so a Markdown decode started
    // by the same host between two landings can never share a number with a canvas picture.
    let mut cache = InlineImageCache::default();
    let mut host = CanvasPictureHost::default();
    let k1 = "canvas://p/1/00000000000000d1";
    host.landed(
        &mut cache,
        answer_of(
            k1,
            CanvasDecoded::Still(image::DynamicImage::new_rgba8(2, 1 + 1)),
        ),
        BUDGET,
    );
    let r1 = cache.cache[Path::new(k1)].request;
    let born = cache.cache[Path::new(k1)].born;
    let md = PathBuf::from("/x/a.png");
    cache.cache.entry(md.clone()).or_default();
    let md_req = cache.next_request(&md);
    let k2 = "canvas://p/1/00000000000000d2";
    host.landed(
        &mut cache,
        answer_of(
            k2,
            CanvasDecoded::Still(image::DynamicImage::new_rgba8(2, 2 + 1)),
        ),
        BUDGET,
    );
    let r2 = cache.cache[Path::new(k2)].request;
    assert!(r1 < md_req && md_req < r2, "{r1} {md_req} {r2}");
    // And a re-created entry of the same key has a new `born`.
    host.forget(&mut cache);
    host.landed(
        &mut cache,
        answer_of(
            k1,
            CanvasDecoded::Still(image::DynamicImage::new_rgba8(2, 1 + 1)),
        ),
        BUDGET,
    );
    assert!(cache.cache[Path::new(k1)].born > born);
}

#[test]
fn a_cancelled_picture_is_not_progress_even_when_more_is_waiting() {
    // `file_decoded` also forgets a cancelled result, so the entry alone cannot tell the guards
    // apart: the landing must not count it as an added picture (that would ask for another answer
    // on the strength of nothing).
    let key = "canvas://p/1/00000000000000d3";
    let mut cache = InlineImageCache::default();
    let mut host = CanvasPictureHost::default();
    let mut got = answer_of(key, CanvasDecoded::Failed(ImageFailure::Cancelled));
    got.more = true;
    got.waiting = 3;
    let landed = host.landed(&mut cache, got, BUDGET);
    assert!(!landed.ask_again, "a cancelled decode is not progress");
}

#[test]
fn landing_keeps_the_hosts_other_pictures_within_the_budget() {
    // The budget is the host's bound on its whole cache: after a canvas picture is filed the
    // pictures that can be rebuilt (here a plain file picture) give their pixels back, and the
    // canvas pictures themselves (whose bytes cannot be rebuilt) stay.
    let mut cache = InlineImageCache::default();
    let other = PathBuf::from("/x/other.png");
    let e = cache.cache.entry(other.clone()).or_default();
    e.decoded = Some(Arc::new(image::DynamicImage::new_rgba8(64, 64)));
    e.last_used = 0;
    // A draw pass has happened since the other picture was used.
    cache.begin_pass();
    cache.begin_pass();
    let key = "canvas://p/1/00000000000000d4";
    let mut host = CanvasPictureHost::default();
    host.landed(
        &mut cache,
        answer_of(
            key,
            CanvasDecoded::Still(image::DynamicImage::new_rgba8(8, 8)),
        ),
        0,
    );
    assert!(
        cache.cache[&other].decoded.is_none(),
        "rebuildable pixels given back"
    );
    assert!(cache.cache[&other].evicted);
    assert!(
        cache.cache[Path::new(key)].decoded.is_some(),
        "canvas pixels stay"
    );
}

fn placed_picture(host: &mut CanvasPictureHost, cache: &mut InlineImageCache, key: &str) {
    let mut got = answer_of(
        key,
        CanvasDecoded::Still(image::DynamicImage::new_rgba8(4, 4)),
    );
    got.places[0].rect = CellRect::new(1, 1, 2, 2);
    host.landed(cache, got, BUDGET);
}

#[test]
fn a_canvas_without_placements_draws_nothing_and_starts_no_pass() {
    let mut cache = InlineImageCache::default();
    let mut host = CanvasPictureHost::default();
    let area = ratatui::layout::Rect::new(0, 0, 10, 5);
    let mut buf = ratatui::buffer::Buffer::empty(area);
    assert_eq!(draw_images(&mut buf, area, &mut host, &mut cache), None);
    assert_eq!(cache.frame, 0, "no placement, no pass");
}

#[test]
fn a_picture_not_encoded_yet_leaves_the_cells_and_records_no_overlay() {
    let mut cache = InlineImageCache::default();
    let mut host = CanvasPictureHost::default();
    let key = "canvas://p/1/00000000000000e1";
    placed_picture(&mut host, &mut cache, key);
    let area = ratatui::layout::Rect::new(0, 0, 10, 5);
    let mut buf = ratatui::buffer::Buffer::empty(area);
    buf.set_string(
        1,
        1,
        "ab",
        ratatui::style::Style::default().fg(ratatui::style::Color::Red),
    );
    let before = buf.clone();
    // No encode worker: the picture has pixels but nothing to draw yet.
    assert_eq!(draw_images(&mut buf, area, &mut host, &mut cache), None);
    assert_eq!(buf, before, "the plugin's cells stay");
    assert_eq!(cache.frame, 1, "one pass for the frame");
    // The entry was stamped as drawn by that pass (it keeps its encodes from being recycled).
    assert_eq!(cache.cache[Path::new(key)].last_used, 1);
    // Another frame is another pass.
    draw_images(&mut buf, area, &mut host, &mut cache);
    assert_eq!(cache.frame, 2);
    assert_eq!(cache.cache[Path::new(key)].last_used, 2);
}

#[test]
fn a_picture_outside_the_area_is_not_asked_for() {
    let mut cache = InlineImageCache::default();
    let mut host = CanvasPictureHost::default();
    let key = "canvas://p/1/00000000000000e2";
    placed_picture(&mut host, &mut cache, key);
    // The area is above the placement's rows entirely: skipped before anything is stamped.
    let area = ratatui::layout::Rect::new(0, 0, 10, 1);
    let mut buf = ratatui::buffer::Buffer::empty(area);
    assert_eq!(draw_images(&mut buf, area, &mut host, &mut cache), None);
    assert_eq!(cache.cache[Path::new(key)].last_used, 0, "not stamped");
}
