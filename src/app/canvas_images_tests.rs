//! The worker side of canvas pictures: keys, hashes, what is decoded and what is skipped, limits.

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
    CanvasImage {
        id,
        x: 0,
        y: 0,
        cols: 2,
        rows: 2,
        image: data,
    }
}

fn decode(plugin: &str, c: &CanvasView, known: &HashSet<PathBuf>) -> CanvasPictures {
    decode_canvas(plugin, c, known, Path::new("/x/a.spr"), 800, None, false)
}

#[test]
fn key_is_scoped_to_the_plugin_and_the_image_id_and_follows_the_content() {
    let c = canvas_of(vec![im(1, rgba(4, 4, 10))]);
    let a = decode("pa", &c, &HashSet::new()).keys;
    let a2 = decode("pa", &c, &HashSet::new()).keys;
    let b = decode("pb", &c, &HashSet::new()).keys;
    assert_eq!(a, a2, "same plugin, id and content: the same key");
    assert_ne!(a, b, "another plugin: another key");
    let other_id = canvas_of(vec![im(2, rgba(4, 4, 10))]);
    assert_ne!(a, decode("pa", &other_id, &HashSet::new()).keys);
    let other_content = canvas_of(vec![im(1, rgba(4, 4, 11))]);
    assert_ne!(a, decode("pa", &other_content, &HashSet::new()).keys);
    let other_size = canvas_of(vec![im(1, rgba(4, 5, 10))]);
    assert_ne!(a, decode("pa", &other_size, &HashSet::new()).keys);
    assert!(a[0].to_string_lossy().starts_with("canvas://pa/1/"));
}

#[test]
fn a_known_key_is_not_decoded_again() {
    let c = canvas_of(vec![im(1, rgba(4, 4, 77))]);
    let first = decode("pk", &c, &HashSet::new());
    assert_eq!(first.fresh.len(), 1);
    let known: HashSet<PathBuf> = first.keys.iter().cloned().collect();
    let again = decode("pk", &c, &known);
    assert_eq!(again.keys, first.keys);
    assert!(again.fresh.is_empty(), "nothing decoded for a held picture");
    // A new picture next to a held one: only the new one.
    let c2 = canvas_of(vec![im(1, rgba(4, 4, 77)), im(2, rgba(4, 4, 78))]);
    let both = decode("pk", &c2, &known);
    assert_eq!(both.keys.len(), 2);
    assert_eq!(both.fresh.len(), 1);
    assert_eq!(both.fresh[0].0, both.keys[1]);
}

#[test]
fn decoded_count_records_only_real_decodes() {
    let data = rgba(3, 3, 123);
    let hash = content_hash(&data);
    let c = canvas_of(vec![im(1, data.clone()), im(1, data)]);
    let before = seen_for_tests::decoded_count(hash);
    let first = decode("pc", &c, &HashSet::new());
    assert_eq!(
        seen_for_tests::decoded_count(hash),
        before + 1,
        "twice listed, decoded once"
    );
    let known: HashSet<PathBuf> = first.keys.iter().cloned().collect();
    decode("pc", &c, &known);
    assert_eq!(seen_for_tests::decoded_count(hash), before + 1);
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
fn hash_tells_the_kinds_apart() {
    let bytes: Arc<[u8]> = Arc::from(&b"abcd"[..]);
    let enc = content_hash(&ImageData::Encoded(bytes.clone()));
    let svg = content_hash(&ImageData::Svg {
        data: bytes.clone(),
        base_dir: None,
        trusted: false,
    });
    let svg_trusted = content_hash(&ImageData::Svg {
        data: bytes,
        base_dir: None,
        trusted: true,
    });
    assert_ne!(enc, svg);
    assert_ne!(svg, svg_trusted, "a changed claim is another picture");
    let frames_a = content_hash(&ImageData::Frames(vec![(
        rgba(2, 2, 1),
        Duration::from_millis(10),
    )]));
    let frames_b = content_hash(&ImageData::Frames(vec![(
        rgba(2, 2, 1),
        Duration::from_millis(20),
    )]));
    assert_ne!(frames_a, frames_b, "the delay is part of the picture");
}

#[test]
fn a_picture_beyond_the_count_limit_fails_as_too_large() {
    let images: Vec<CanvasImage> = (0..MAX_CANVAS_IMAGES as u64 + 3)
        .map(|i| im(i, rgba(1, 1, (i % 250) as u8 + 1)))
        .collect();
    let got = decode("plim", &canvas_of(images), &HashSet::new());
    assert_eq!(
        got.keys.len(),
        MAX_CANVAS_IMAGES + 3,
        "every picture has a key"
    );
    let failed: Vec<_> = got
        .fresh
        .iter()
        .filter(|(_, d)| {
            matches!(
                d,
                CanvasDecoded::Failed(crate::preview::image::ImageFailure::TooLarge)
            )
        })
        .collect();
    assert!(
        failed.len() >= 3,
        "the ones past the limit failed: {}",
        failed.len()
    );
}

#[test]
fn the_byte_budget_of_one_answer_refuses_what_does_not_fit() {
    // 64 MiB each (4096 x 4096 x 4): the third does not fit the 128 MiB budget.
    let side = 4096u32;
    let mk = |c| {
        let mut px = Vec::new();
        px.resize((side * side * 4) as usize, c);
        ImageData::Rgba {
            width: side,
            height: side,
            pixels: Arc::new(px),
        }
    };
    let got = decode(
        "pbudget",
        &canvas_of(vec![im(1, mk(1)), im(2, mk(2)), im(3, mk(3))]),
        &HashSet::new(),
    );
    let kinds: Vec<bool> = got
        .fresh
        .iter()
        .map(|(_, d)| matches!(d, CanvasDecoded::Still(_)))
        .collect();
    assert_eq!(
        kinds,
        vec![true, true, false],
        "two fit, the third is refused"
    );
}

#[test]
fn frames_decode_as_an_animation_and_a_bad_frame_fails_the_whole_picture() {
    let ok = ImageData::Frames(vec![
        (rgba(2, 2, 1), Duration::from_millis(30)),
        (rgba(2, 2, 2), Duration::from_millis(40)),
    ]);
    let got = decode("pf", &canvas_of(vec![im(1, ok)]), &HashSet::new());
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
    let got = decode("pf", &canvas_of(vec![im(1, bad)]), &HashSet::new());
    assert!(matches!(got.fresh[0].1, CanvasDecoded::Failed(_)));
    // A frame list inside a frame list is not an animation of animations.
    let nested = ImageData::Frames(vec![(
        ImageData::Frames(vec![(rgba(1, 1, 1), Duration::ZERO)]),
        Duration::ZERO,
    )]);
    let got = decode("pf", &canvas_of(vec![im(1, nested)]), &HashSet::new());
    assert!(matches!(got.fresh[0].1, CanvasDecoded::Failed(_)));
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
        &HashSet::new(),
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
        &HashSet::new(),
    );
    assert!(matches!(got.fresh[0].1, CanvasDecoded::Still(_)));
}

#[test]
fn a_cancelled_job_decodes_nothing() {
    let cancel = crate::preview::office::Cancel::new(|| true);
    let c = canvas_of(vec![im(1, rgba(9, 9, 9))]);
    let got = decode_canvas(
        "pcx",
        &c,
        &HashSet::new(),
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
fn a_cancelled_picture_leaves_no_entry_behind() {
    let dir = crate::test_support::unique_tmp("cvi_cancelled");
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = App::new(dir.to_path_buf(), crate::config::Config::default()).unwrap();
    let key = PathBuf::from("canvas://p/1/00000000000000aa");
    app.canvas_pictures_landed(CanvasPictures {
        keys: vec![key.clone()],
        fresh: vec![(
            key.clone(),
            CanvasDecoded::Failed(crate::preview::image::ImageFailure::Cancelled),
        )],
    });
    assert!(
        !app.md_image_cache.contains_key(&key),
        "asked for again next time"
    );
}

#[test]
fn landing_inserts_prunes_and_forgets() {
    let dir = crate::test_support::unique_tmp("cvi_landing");
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = App::new(dir.to_path_buf(), crate::config::Config::default()).unwrap();
    let c1 = canvas_of(vec![im(1, rgba(4, 4, 1)), im(2, rgba(4, 4, 2))]);
    let p1 = decode("pl", &c1, &HashSet::new());
    let keys1 = p1.keys.clone();
    app.canvas_pictures_landed(p1);
    assert_eq!(app.md_image_cache.len(), 2);
    assert_eq!(app.canvas_keys, keys1);
    // An unrelated (Markdown / media-diff) entry is not touched by a canvas landing.
    app.md_image_cache
        .insert(PathBuf::from("media-diff://o/00/1"), Default::default());
    // The next answer keeps picture 1, replaces picture 2's content, adds none.
    let c2 = canvas_of(vec![im(1, rgba(4, 4, 1)), im(2, rgba(4, 4, 3))]);
    let known: HashSet<PathBuf> = app
        .canvas_known_keys()
        .unwrap_or_else(|| app.md_image_cache.keys().cloned().collect());
    let p2 = decode("pl", &c2, &known);
    assert_eq!(p2.fresh.len(), 1, "only the changed picture");
    let keys2 = p2.keys.clone();
    app.canvas_pictures_landed(p2);
    assert_eq!(keys2[0], keys1[0]);
    assert_ne!(keys2[1], keys1[1]);
    assert!(app.md_image_cache.contains_key(&keys1[0]));
    assert!(
        !app.md_image_cache.contains_key(&keys1[1]),
        "the replaced one is dropped"
    );
    assert!(app.md_image_cache.contains_key(&keys2[1]));
    assert!(app
        .md_image_cache
        .contains_key(&PathBuf::from("media-diff://o/00/1")));
    // A canvas without pictures drops them all.
    app.canvas_pictures_landed(decode("pl", &canvas_of(vec![]), &HashSet::new()));
    assert!(app
        .md_image_cache
        .keys()
        .all(|k| !crate::preview::markdown::is_canvas_url(&k.to_string_lossy())));
    assert!(app.canvas_keys.is_empty());
    // And forgetting clears whatever is left.
    app.canvas_pictures_landed(decode("pl", &c1, &HashSet::new()));
    app.forget_canvas_pictures();
    assert!(app.canvas_keys.is_empty());
    assert!(app
        .md_image_cache
        .keys()
        .all(|k| !crate::preview::markdown::is_canvas_url(&k.to_string_lossy())));
}

#[test]
fn a_held_entry_is_kept_when_a_late_answer_decoded_it_again() {
    let dir = crate::test_support::unique_tmp("cvi_late");
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = App::new(dir.to_path_buf(), crate::config::Config::default()).unwrap();
    let c = canvas_of(vec![im(1, rgba(4, 4, 5))]);
    app.canvas_pictures_landed(decode("pq", &c, &HashSet::new()));
    let key = app.canvas_keys[0].clone();
    let request = app.md_request_for_test(&key);
    // An answer built before the first landed carries the same picture decoded once more.
    app.canvas_pictures_landed(decode("pq", &c, &HashSet::new()));
    assert_eq!(
        app.md_request_for_test(&key),
        request,
        "the entry (and its encodes) stayed"
    );
    assert_eq!(app.md_image_cache.len(), 1);
}

#[test]
fn no_known_keys_without_an_image_backend() {
    let dir = crate::test_support::unique_tmp("cvi_nobackend");
    std::fs::create_dir_all(&dir).unwrap();
    let app = App::new(dir.to_path_buf(), crate::config::Config::default()).unwrap();
    assert!(app.canvas_known_keys().is_none());
    assert!(!app.canvas_pictures_drawable());
}

#[test]
fn canvas_entries_are_never_evicted_by_the_cache_budget() {
    // Their pixels cannot be rebuilt (the bytes were the plugin's), so the budget must not drop them.
    let dir = crate::test_support::unique_tmp("cvi_noevict");
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = App::new(dir.to_path_buf(), crate::config::Config::default()).unwrap();
    app.canvas_pictures_landed(decode(
        "pe",
        &canvas_of(vec![im(1, rgba(64, 64, 9))]),
        &HashSet::new(),
    ));
    let key = app.canvas_keys[0].clone();
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
