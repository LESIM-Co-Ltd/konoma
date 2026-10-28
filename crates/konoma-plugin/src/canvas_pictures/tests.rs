//! The decoding of canvas pictures: keys and hashes, what is decoded and what waits, the limits,
//! the read scope. (The tests that need the host's App, its supervised SVG child or its kitty
//! cache are in the host.)

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::budget::{decode_cost, header_pixels, header_pixels_within, input_bytes};
use super::files::MAX_PICTURE_FILE_BYTES;
use super::hash::{same_source, Fnv, Hasher, HASH_CHUNK};
use super::seen_for_tests::decoded_count;
use super::*;
use crate::view::{CanvasImage, CanvasView};

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

/// An SVG drawer for tests of what is not SVG: these tests draw none, so a draw is a failure.
struct NoSvg;

impl SvgDraw for NoSvg {
    fn untrusted(
        &self,
        _: &[u8],
        _: &Path,
        _: Option<&[PathBuf]>,
        _: u32,
        _: &dyn Fn() -> bool,
    ) -> Result<image::DynamicImage, konoma_svg::svg::RasterError> {
        panic!("a test that draws no SVG drew one")
    }

    fn embedded(
        &self,
        _: &[u8],
        _: u32,
        _: &dyn Fn() -> bool,
    ) -> Result<image::DynamicImage, konoma_svg::svg::RasterError> {
        panic!("a test that draws no SVG drew one")
    }
}

fn dc(
    plugin: &str,
    c: &CanvasView,
    held: &CanvasHeld,
    shown: &Path,
    scope: &crate::ReadScope,
) -> CanvasPictures {
    dcl(plugin, c, held, shown, scope, CanvasLimits::PRODUCTION)
}

fn dcl(
    plugin: &str,
    c: &CanvasView,
    held: &CanvasHeld,
    shown: &Path,
    scope: &crate::ReadScope,
    limits: CanvasLimits,
) -> CanvasPictures {
    let policy = CanvasPolicy::new(scope, &NoSvg, 800).limits(limits);
    decode_canvas(plugin, c, held, shown, &policy)
}

fn decode(plugin: &str, c: &CanvasView, held: &CanvasHeld) -> CanvasPictures {
    dc(
        plugin,
        c,
        held,
        Path::new("/x/a.spr"),
        &crate::ReadScope::unrestricted(),
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
        &crate::ReadScope::unrestricted(),
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

// ---- batch F: the bytes counted ----------------------------------------------------------------------

fn png_of(img: image::DynamicImage) -> ImageData {
    let mut out = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
        .unwrap();
    ImageData::Encoded(Arc::from(out))
}

// ---- round 3: a new content replaces the old one of its id; the asks; the kept bound -----------------

// ---- round 3 of the mutation audit: survivors of the pictures' worker and landing --------------------

fn junk(n: u8) -> ImageData {
    ImageData::Encoded(Arc::from(vec![n; 100]))
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

/// An animation of `n` frames of `w` x `h` pixels.
fn animation_of(n: usize, w: u32, h: u32) -> ImageData {
    ImageData::Frames(
        (0..n)
            .map(|i| (rgba(w, h, i as u8), Duration::from_millis(100)))
            .collect(),
    )
}

// ---- picture files (S7) ------------------------------------------------------------------------

fn write_png(p: &Path, c: u8) {
    image::RgbaImage::from_pixel(3, 3, image::Rgba([c, 2, 3, 255]))
        .save(p)
        .unwrap();
}

fn enc_png() -> ImageData {
    png_of(image::DynamicImage::new_rgba8(2, 2))
}

// ---- the read scope of a plugin the user added (canvas pictures) ------------------------------------

fn scoped(dirs: &[&Path]) -> crate::ReadScope {
    crate::ReadScope::within(dirs.iter().copied())
}

fn canvas_file(p: &Path, shown: &Path, scope: &crate::ReadScope) -> CanvasDecoded {
    let c = canvas_of(vec![im(1, ImageData::File(p.to_path_buf()))]);
    let mut got = dc("pr", &c, &nothing_held(), shown, scope);
    assert_eq!(got.fresh.len(), 1);
    got.fresh.remove(0).1
}

// ---- read scope ------------------------------------------------------------------------------------

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

/// The hash the worker names a picture by: whether the picture was decoded is read from it.
fn decoded_times(data: &ImageData) -> usize {
    decoded_count(content_hash(data))
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
fn pictures_with_the_same_pixel_bytes_but_another_shape_get_other_keys() {
    // 64 bytes of the same pixels as 2x8, 4x4, 8x2, 1x16 and 16x1: five pictures, five keys.
    let shapes = [(2, 8), (4, 4), (8, 2), (1, 16), (16, 1)];
    let mut seen = HashSet::new();
    for (w, h) in shapes {
        let c = canvas_of(vec![im(1, rgba(w, h, 9))]);
        let k = keys(&decode("ps", &c, &nothing_held()));
        assert!(seen.insert(k), "{w}x{h} shares a key with another shape");
    }
    // Decoded, each keeps its own shape (the second is not served the first's decode).
    let two = canvas_of(vec![im(1, rgba(2, 8, 9)), im(2, rgba(4, 4, 9))]);
    let got = decode("ps", &two, &nothing_held());
    assert_eq!(got.fresh.len(), 2, "two pictures, two decodes");
    let dims: Vec<(u32, u32)> = got
        .fresh
        .iter()
        .map(|(_, d)| match d {
            CanvasDecoded::Still(i) => (i.width(), i.height()),
            _ => panic!("decoded as a still"),
        })
        .collect();
    assert_eq!(dims, vec![(2, 8), (4, 4)]);
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
    let dir = konoma_test_support::unique_tmp("cvi_filehash");
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
    let dir = konoma_test_support::unique_tmp("cvi_filemtime");
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
fn file_hash_follows_the_inode_and_the_change_time_with_size_and_modification_time_kept() {
    use std::os::unix::fs::MetadataExt;
    let dir = konoma_test_support::unique_tmp("cvi_fileinode");
    std::fs::create_dir_all(&dir).unwrap();
    let f = dir.join("p.png");
    let t0 = std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let put_back = |p: &Path| {
        std::fs::OpenOptions::new()
            .write(true)
            .open(p)
            .unwrap()
            .set_modified(t0)
            .unwrap();
    };
    std::fs::write(&f, b"aaaa").unwrap();
    put_back(&f);
    let a = content_hash(&ImageData::File(f.to_path_buf()));
    // Replaced by another file of the same size with the same modification time (what `cp -p`,
    // `rsync -t` and a rename do): another inode.
    let other = dir.join("other.png");
    std::fs::write(&other, b"bbbb").unwrap();
    put_back(&other);
    std::fs::rename(&other, &f).unwrap();
    let b = content_hash(&ImageData::File(f.to_path_buf()));
    assert_ne!(a, b, "a replacement with the size and the time kept");
    // Rewritten in place, the time put back: the inode is the same, the change time moved.
    let ino = std::fs::metadata(&f).unwrap().ino();
    let mut before = std::fs::metadata(&f).unwrap().ctime_nsec();
    let mut moved = false;
    for _ in 0..200 {
        std::fs::write(&f, b"cccc").unwrap();
        put_back(&f);
        let m = std::fs::metadata(&f).unwrap();
        assert_eq!(m.ino(), ino);
        if m.ctime_nsec() != before {
            moved = true;
            break;
        }
        before = m.ctime_nsec();
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(moved, "the file system never moved the change time");
    assert_ne!(b, content_hash(&ImageData::File(f.to_path_buf())));
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
        crate::view::CANVAS_MAX_PICTURES,
        MAX_CANVAS_IMAGES,
        "the API's distinct-id limit is the host's"
    );
    assert_eq!(MAX_HEADER_PIXELS_PER_ANSWER, 400_000_000);
    assert_eq!(
        MAX_HEADER_PIXELS_PER_ANSWER,
        konoma_svg::image::MAX_GIF_WORK_PIXELS,
        "the number the GIF decoder is calibrated with"
    );
    let p = CanvasLimits::PRODUCTION;
    assert_eq!(
        (
            p.held_bytes,
            p.answer_bytes,
            p.decodes,
            p.input_bytes,
            p.header_pixels
        ),
        (
            MAX_CANVAS_HELD_BYTES,
            MAX_CANVAS_BYTES,
            MAX_DECODES_PER_ANSWER,
            MAX_INPUT_BYTES_PER_ANSWER,
            MAX_HEADER_PIXELS_PER_ANSWER
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
fn the_memory_bound_counts_the_entry_of_another_id_but_not_the_one_its_new_content_replaces() {
    // Id 1 is drawn from an old 200,000-byte entry while its new content comes: the new content
    // replaces it, so the old entry is not held next to it. Id 2 (unchanged, in the cache) is held
    // whatever happens: 200,000 bytes next to the new 40,000.
    let mut held = nothing_held();
    let old: Arc<str> = Arc::from("canvas://pw/1/00000000000000aa");
    held.keys.insert(PathBuf::from(&*old), 200_000);
    held.front.insert(1, old);
    let other: Arc<str> = Arc::from("canvas://pw/2/00000000000000bb");
    held.keys.insert(PathBuf::from(&*other), 200_000);
    held.by_id.insert(2, other.clone());
    held.front.insert(2, other);
    let c = canvas_of(vec![im(1, rgba(100, 100, 1)), none(2)]);
    let tight = decode_within(&c, &held, small(|l| l.held_bytes = 230_000));
    assert!(
        tight.fresh.is_empty(),
        "200,000 of id 2 + 40,000 > 230,000, though id 1's old entry is not counted"
    );
    assert_eq!(tight.refused, 1);
    let roomy = decode_within(&c, &held, small(|l| l.held_bytes = 240_000));
    assert_eq!(roomy.fresh.len(), 1, "exactly the bound fits");
    // Had the old entry of id 1 counted, 440,000 would not fit 240,000.
}

#[test]
fn a_picture_that_can_never_fit_fails_for_good() {
    let c = canvas_of(vec![im(1, rgba(100, 100, 1))]);
    let got = decode_within(&c, &nothing_held(), small(|l| l.held_bytes = 39_999));
    assert!(matches!(
        got.fresh[0].1,
        CanvasDecoded::Failed(konoma_svg::image::ImageFailure::TooLarge)
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
            CanvasDecoded::Failed(konoma_svg::image::ImageFailure::TooLarge)
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
            konoma_svg::image::ImageFailure::Corrupt
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

#[test]
fn only_a_regular_file_is_opened_for_a_plugin() {
    let dir = konoma_test_support::unique_tmp("cvi_regular");
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

#[test]
fn the_apis_rgba_side_limit_is_the_decode_cap_of_the_host() {
    assert_eq!(
        crate::view::CANVAS_MAX_RGBA_SIDE,
        konoma_widgets::inline_images::INLINE_IMAGE_MAX_SIDE
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
    let mut h = Hasher::new(&|| false);
    let hs = h.content(&same).unwrap();
    assert_eq!(h.bytes, 100_000, "one buffer, hashed once");
    let mut h2 = Hasher::new(&|| false);
    let ha = h2.content(&apart).unwrap();
    assert_eq!(h2.bytes, 50 * 100_000, "distinct buffers are each hashed");
    assert_eq!(hs, ha, "the identity of a buffer is not part of the hash");
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

#[test]
fn a_file_of_another_size_is_another_picture_even_with_the_same_modification_time() {
    let dir = konoma_test_support::unique_tmp("cvi_filesize");
    std::fs::create_dir_all(&dir).unwrap();
    let f = dir.join("p.png");
    let t0 = std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let set = || {
        std::fs::OpenOptions::new()
            .write(true)
            .open(&f)
            .unwrap()
            .set_modified(t0)
            .unwrap();
    };
    std::fs::write(&f, b"aaaa").unwrap();
    set();
    let a = content_hash(&ImageData::File(f.to_path_buf()));
    std::fs::write(&f, b"aaaaaa").unwrap();
    set();
    let b = content_hash(&ImageData::File(f.to_path_buf()));
    assert_ne!(a, b, "same path, same time, another size");
}

#[test]
fn the_same_pixels_in_another_shape_are_not_the_same_source() {
    let p = Arc::new(vec![0u8; 64]);
    let r = |w: u32, h: u32| ImageData::Rgba {
        width: w,
        height: h,
        pixels: p.clone(),
    };
    assert!(same_source(&r(4, 4), &r(4, 4)));
    assert!(!same_source(&r(4, 4), &r(2, 8)), "both differ");
    assert!(!same_source(&r(4, 4), &r(4, 8)), "the height differs");
    assert!(!same_source(&r(4, 4), &r(8, 4)), "the width differs");
    // Through the worker: the hash made for the 4 x 4 picture is not reused for the 2 x 8 one that
    // shares its buffer, so the key follows the new shape.
    let first = decode("pshape", &canvas_of(vec![im(1, r(4, 4))]), &nothing_held());
    let key = first.places[0].key.clone().unwrap();
    let mut held = nothing_held();
    held.kept.insert(
        1,
        KeptPicture {
            key: Some(key.clone()),
            hash: Some(content_hash(&r(4, 4))),
            data: Arc::new(r(4, 4)),
            needs: None,
        },
    );
    let next = decode("pshape", &canvas_of(vec![im(1, r(2, 8))]), &held);
    assert_ne!(next.places[0].key.as_deref(), Some(&*key));
}

#[test]
fn files_count_against_the_input_budget_of_the_decode_loop() {
    // A file is hashed by its path (no bytes counted), so only the decode loop can stop on the
    // bytes it reads: three 600-byte files against a 1000-byte budget -- two run, the third waits.
    let dir = konoma_test_support::unique_tmp("cvi_fileinput");
    std::fs::create_dir_all(&dir).unwrap();
    let files: Vec<ImageData> = (1..=3u8)
        .map(|n| {
            let f = dir.join(format!("f{n}.bin"));
            std::fs::write(&f, vec![n; 600]).unwrap();
            ImageData::File(f.to_path_buf())
        })
        .collect();
    let c = canvas_of(
        files
            .into_iter()
            .enumerate()
            .map(|(i, d)| im(i as u64 + 1, d))
            .collect(),
    );
    let got = decode_within(&c, &nothing_held(), small(|l| l.input_bytes = 1000));
    assert_eq!(got.fresh.len(), 2);
    assert_eq!(got.waiting, 1);
    assert!(got.more);
    assert!(
        got.places.iter().all(|p| p.key.is_some()),
        "all three were hashed (by their path)"
    );
}

#[test]
fn sources_the_host_kept_unhashed_are_hashed_before_new_pictures_when_the_budget_is_short() {
    let mut held = nothing_held();
    held.kept.insert(
        2,
        KeptPicture {
            key: None,
            hash: None,
            data: Arc::new(junk(2)),
            needs: None,
        },
    );
    // Id 1 is new and comes first in the canvas; id 2 was kept. A budget of one byte hashes one
    // picture (the first is always hashed): the kept one.
    let c = canvas_of(vec![im(1, junk(1)), none(2)]);
    let got = decode_within(&c, &held, small(|l| l.input_bytes = 1));
    assert!(got.places[1].key.is_some(), "the kept source is hashed");
    assert!(got.places[0].key.is_none(), "the new picture waits");
    assert_eq!(got.waiting, 1);
}

#[test]
fn pictures_are_decoded_in_the_order_they_appear_though_kept_ones_were_hashed_first() {
    let kept_data = rgba(4, 4, 9);
    let hash = content_hash(&kept_data);
    let key: Arc<str> = konoma_widgets::markdown::canvas_url("pw", 2, hash).into();
    let mut held = nothing_held();
    held.kept.insert(
        2,
        KeptPicture {
            key: Some(key.clone()),
            hash: Some(hash),
            data: Arc::new(kept_data),
            needs: None,
        },
    );
    // One decode per answer: the picture that appears first in the canvas (id 1, new) goes, the
    // kept one (id 2, listed second) waits for the next answer.
    let c = canvas_of(vec![im(1, rgba(4, 4, 5)), none(2)]);
    let got = decode_within(&c, &held, small(|l| l.decodes = 1));
    assert_eq!(got.fresh.len(), 1);
    assert_eq!(
        got.fresh[0].0,
        PathBuf::from(&**got.places[0].key.as_ref().unwrap()),
        "the first in the canvas"
    );
    assert_eq!(got.waiting, 1);
    assert_eq!(got.places[1].key.as_deref(), Some(&*key));
}

#[test]
fn a_copy_that_exactly_fits_the_room_is_taken() {
    let data = rgba(100, 100, 9); // 40,000 bytes
    let c = canvas_of(vec![im(1, data.clone()), im(2, data)]);
    let exact = decode_within(&c, &nothing_held(), small(|l| l.held_bytes = 80_000));
    assert_eq!(exact.fresh.len(), 2, "40,000 + 40,000 fills 80,000");
    assert_eq!(exact.refused, 0);
    let short = decode_within(&c, &nothing_held(), small(|l| l.held_bytes = 79_999));
    assert_eq!(short.fresh.len(), 1);
    assert_eq!(short.refused, 1);
}

#[test]
fn the_first_picture_is_hashed_even_with_no_input_budget_at_all() {
    let c = canvas_of(vec![im(1, junk(1)), im(2, junk(2))]);
    let got = decode_within(&c, &nothing_held(), small(|l| l.input_bytes = 0));
    assert!(
        got.places[0].key.is_some(),
        "an answer always makes progress"
    );
    assert!(got.places[1].key.is_none());
    assert_eq!(got.waiting, 2, "one unhashed, one hashed but not decoded");
}

// ---- round 3: the boundaries of the limits, exactly at the bound ---------------------------------------

#[test]
fn a_picture_exactly_as_big_as_the_memory_bound_fits() {
    let c = canvas_of(vec![im(1, rgba(100, 100, 1))]); // 40,000 bytes
    let exact = decode_within(&c, &nothing_held(), small(|l| l.held_bytes = 40_000));
    assert!(matches!(exact.fresh[0].1, CanvasDecoded::Still(_)));
    assert_eq!(exact.refused, 0);
}

#[test]
fn a_refused_picture_is_decoded_again_when_the_room_is_exactly_what_it_needs() {
    let data = rgba(100, 100, 3);
    let hash = content_hash(&data);
    let key: Arc<str> = konoma_widgets::markdown::canvas_url("pw", 1, hash).into();
    let kept = |needs: u64| {
        let mut held = nothing_held();
        held.kept.insert(
            1,
            KeptPicture {
                key: Some(key.clone()),
                hash: Some(hash),
                data: Arc::new(data.clone()),
                needs: Some(needs),
            },
        );
        held
    };
    let c = canvas_of(vec![none(1)]);
    let l = small(|l| l.held_bytes = 40_000);
    let exact = decode_within(&c, &kept(40_000), l);
    assert_eq!(exact.fresh.len(), 1, "the room is what it needs");
    assert_eq!(exact.refused, 0);
    let short = decode_within(&c, &kept(40_001), l);
    assert!(short.fresh.is_empty(), "one byte short: not decoded again");
    assert_eq!(short.refused, 1);
}

#[test]
fn the_input_budget_exactly_used_leaves_the_next_picture_unhashed() {
    let c = canvas_of(vec![im(1, junk(1)), im(2, junk(2))]); // 100 bytes each
    let got = decode_within(&c, &nothing_held(), small(|l| l.input_bytes = 100));
    assert!(got.places[0].key.is_some());
    assert!(got.places[1].key.is_none(), "100 bytes were read of 100");
    let more = decode_within(&c, &nothing_held(), small(|l| l.input_bytes = 101));
    assert!(more.places[1].key.is_some());
}

#[test]
fn an_unchanged_picture_keeps_the_kept_allocation_through_the_worker() {
    // The worker hands back the source of a kept picture it decodes: the same allocation, not a
    // deep copy of its frames.
    let data = Arc::new(animation(50, 1));
    let mut held = nothing_held();
    held.kept.insert(
        1,
        KeptPicture {
            key: None,
            hash: None,
            data: data.clone(),
            needs: None,
        },
    );
    let c = canvas_of(vec![none(1)]);
    // The source kept for the landing is the same allocation, not a copy.
    let got = decode_within(&c, &held, CanvasLimits::PRODUCTION);
    let (_, kept) = got.sources.iter().find(|(id, _)| *id == 1).unwrap();
    assert!(Arc::ptr_eq(&kept.data, &data), "no copy of the frames");
}

#[test]
fn an_animation_with_more_input_than_the_budget_is_refused_but_a_single_image_is_not() {
    // Frames of 100x100 pixels (40,000 bytes each) against an input budget of 100,000: three
    // frames are 120,000 > 100,000 and are refused as one animation; one single image of 40,000
    // bytes is decoded as the first of an answer even when the budget is 1 byte (an answer
    // makes progress).
    let big = canvas_of(vec![im(1, animation_of(3, 100, 100))]);
    let got = decode_within(&big, &nothing_held(), small(|l| l.input_bytes = 100_000));
    assert!(matches!(
        got.fresh[0].1,
        CanvasDecoded::Failed(konoma_svg::image::ImageFailure::TooLarge)
    ));
    let ok = canvas_of(vec![im(1, animation_of(2, 100, 100))]);
    let got = decode_within(&ok, &nothing_held(), small(|l| l.input_bytes = 100_000));
    assert!(matches!(got.fresh[0].1, CanvasDecoded::Frames(_)));
    let single = canvas_of(vec![im(1, rgba(100, 100, 3))]);
    let got = decode_within(&single, &nothing_held(), small(|l| l.input_bytes = 1));
    assert!(matches!(got.fresh[0].1, CanvasDecoded::Still(_)));
}

#[test]
fn a_relative_picture_path_is_relative_to_the_previewed_files_directory() {
    let dir = konoma_test_support::unique_tmp("cvi_rel");
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    write_png(&dir.join("sub").join("pic.png"), 9);
    let shown = dir.join("a.spr");
    // The process's own directory is somewhere else: the path must not be looked for there.
    let c = canvas_of(vec![im(1, ImageData::File("sub/pic.png".into()))]);
    let got = dc(
        "pr",
        &c,
        &nothing_held(),
        &shown,
        &crate::ReadScope::unrestricted(),
    );
    assert!(
        matches!(got.fresh[0].1, CanvasDecoded::Still(_)),
        "found next to the shown file"
    );
    // An absolute path is itself, wherever the shown file is.
    let abs = canvas_of(vec![im(
        1,
        ImageData::File(dir.join("sub").join("pic.png")),
    )]);
    let got = dc(
        "pr",
        &abs,
        &nothing_held(),
        Path::new("/elsewhere/b.spr"),
        &crate::ReadScope::unrestricted(),
    );
    assert!(matches!(got.fresh[0].1, CanvasDecoded::Still(_)));
    // A relative path that is not there fails (it is not looked for in the working directory).
    let missing = canvas_of(vec![im(1, ImageData::File("nothing.png".into()))]);
    let got = dc(
        "pr",
        &missing,
        &nothing_held(),
        &shown,
        &crate::ReadScope::unrestricted(),
    );
    assert!(matches!(got.fresh[0].1, CanvasDecoded::Failed(_)));
}

#[test]
fn the_hash_of_a_relative_file_follows_the_file_next_to_the_shown_one() {
    let dir = konoma_test_support::unique_tmp("cvi_rel_hash");
    std::fs::create_dir_all(&dir).unwrap();
    let shown = dir.join("a.spr");
    let c = canvas_of(vec![im(1, ImageData::File("pic.png".into()))]);
    write_png(&dir.join("pic.png"), 1);
    let first = keys(&dc(
        "pr",
        &c,
        &nothing_held(),
        &shown,
        &crate::ReadScope::unrestricted(),
    ));
    // The file changed under the same name: another picture (its size/mtime are of that file, not
    // of whatever the working directory holds under that name).
    write_png(&dir.join("pic.png"), 200);
    std::fs::OpenOptions::new()
        .write(true)
        .open(dir.join("pic.png"))
        .unwrap()
        .set_modified(std::time::SystemTime::now() + Duration::from_secs(5))
        .unwrap();
    let second = keys(&dc(
        "pr",
        &c,
        &nothing_held(),
        &shown,
        &crate::ReadScope::unrestricted(),
    ));
    assert_ne!(first, second);
}

#[test]
fn a_device_a_directory_and_an_oversized_file_are_refused_on_the_opened_handle() {
    let dir = konoma_test_support::unique_tmp("cvi_special");
    std::fs::create_dir_all(&dir).unwrap();
    let shown = dir.join("a.spr");
    // `/dev/zero` is the telling one: read as a file it would never end (and be refused as too
    // large after 64 MiB); refused on the handle it is "not a file" at once.
    for special in [
        PathBuf::from("/dev/zero"),
        PathBuf::from("/dev/null"),
        dir.to_path_buf(),
    ] {
        let c = canvas_of(vec![im(1, ImageData::File(special.clone()))]);
        let got = dc(
            "pr",
            &c,
            &nothing_held(),
            &shown,
            &crate::ReadScope::unrestricted(),
        );
        assert!(
            matches!(
                got.fresh[0].1,
                CanvasDecoded::Failed(konoma_svg::image::ImageFailure::Corrupt)
            ),
            "{special:?} is not a regular file"
        );
    }
    // Over the cap: refused as too large, without reading it (a sparse file costs nothing).
    let big = dir.join("big.png");
    let f = std::fs::File::create(&big).unwrap();
    f.set_len(MAX_PICTURE_FILE_BYTES + 1).unwrap();
    let c = canvas_of(vec![im(1, ImageData::File(big))]);
    let got = dc(
        "pr",
        &c,
        &nothing_held(),
        &shown,
        &crate::ReadScope::unrestricted(),
    );
    assert!(matches!(
        got.fresh[0].1,
        CanvasDecoded::Failed(konoma_svg::image::ImageFailure::TooLarge)
    ));
}

#[test]
fn a_fifo_that_nobody_writes_to_is_refused_without_blocking_inside_frames_too() {
    // The open itself must not block (`O_NONBLOCK`); the decision is made on the handle.
    let dir = konoma_test_support::unique_tmp("cvi_fifo2");
    std::fs::create_dir_all(&dir).unwrap();
    let fifo = dir.join("pipe.gif");
    if !std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .is_ok_and(|s| s.success())
    {
        return;
    }
    let (tx, rx) = std::sync::mpsc::channel();
    let shown = dir.join("a.spr");
    std::thread::spawn(move || {
        let c = canvas_of(vec![im(1, ImageData::File(fifo))]);
        let got = dc(
            "pr",
            &c,
            &nothing_held(),
            &shown,
            &crate::ReadScope::unrestricted(),
        );
        let _ = tx.send(matches!(got.fresh[0].1, CanvasDecoded::Failed(_)));
    });
    assert!(
        rx.recv_timeout(Duration::from_secs(10))
            .expect("the open blocked"),
        "refused"
    );
}

#[test]
fn a_gif_file_is_an_animation_read_from_the_opened_handle() {
    use image::codecs::gif::GifEncoder;
    let dir = konoma_test_support::unique_tmp("cvi_gif_file");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("anim.gif");
    {
        let mut enc = GifEncoder::new(std::fs::File::create(&path).unwrap());
        for c in [10u8, 200] {
            let frame = image::Frame::new(image::RgbaImage::from_pixel(
                4,
                4,
                image::Rgba([c, 0, 0, 255]),
            ));
            enc.encode_frame(frame).unwrap();
        }
    }
    let c = canvas_of(vec![im(1, ImageData::File("anim.gif".into()))]);
    let got = dc(
        "pg",
        &c,
        &nothing_held(),
        &dir.join("a.spr"),
        &crate::ReadScope::unrestricted(),
    );
    let CanvasDecoded::Frames(f) = &got.fresh[0].1 else {
        panic!("a GIF file with two frames is an animation");
    };
    assert_eq!(f.len(), 2);
}

#[test]
fn an_encoded_frame_costs_a_decode_like_a_file_and_only_a_ready_rgba_frame_costs_none() {
    // An `Encoded` frame runs a full decode of whatever its header admits, however few bytes it
    // is: it is counted like a file or an SVG frame; a ready `Rgba` frame is not.
    let enc = || png_of(image::DynamicImage::new_rgba8(2, 2));
    let d = Duration::from_millis(100);
    let frames = |data: fn() -> ImageData, n: usize| {
        ImageData::Frames((0..n).map(|_| (data(), d)).collect())
    };
    assert_eq!(
        decode_cost(&frames(|| rgba(1, 1, 1), 50)),
        1,
        "at least one"
    );
    assert_eq!(decode_cost(&frames(enc_png, 50)), 50);
    assert_eq!(decode_cost(&enc()), 1);
    let _ = enc;
    // And the budget refuses the 50-frame encoded animation for good against 32 decodes, while 50
    // ready frames are fine.
    let c = canvas_of(vec![im(1, frames(enc_png, 40))]);
    let got = decode_within(&c, &nothing_held(), small(|l| l.decodes = 32));
    assert!(matches!(
        got.fresh[0].1,
        CanvasDecoded::Failed(konoma_svg::image::ImageFailure::TooLarge)
    ));
    let c = canvas_of(vec![im(1, frames(enc_png, 4))]);
    let got = decode_within(&c, &nothing_held(), small(|l| l.decodes = 32));
    assert!(matches!(got.fresh[0].1, CanvasDecoded::Frames(_)));
}

#[test]
fn a_canvas_picture_outside_the_scope_is_refused_with_its_own_reason() {
    let dir = konoma_test_support::unique_tmp("cvi_scope");
    let (root, out) = (dir.join("root"), dir.join("out"));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&out).unwrap();
    write_png(&root.join("in.png"), 1);
    write_png(&out.join("out.png"), 2);
    let scope = scoped(&[&root]);
    let shown = root.join("a.spr");
    assert!(matches!(
        canvas_file(&root.join("in.png"), &shown, &scope),
        CanvasDecoded::Still(_)
    ));
    assert!(matches!(
        canvas_file(&out.join("out.png"), &shown, &scope),
        CanvasDecoded::Failed(konoma_svg::image::ImageFailure::OutsideReadDirs)
    ));
    // The same file under a widened scope.
    let wide = scoped(&[&root, &out]);
    assert!(matches!(
        canvas_file(&out.join("out.png"), &shown, &wide),
        CanvasDecoded::Still(_)
    ));
}

#[test]
fn a_canvas_picture_that_is_the_previewed_file_is_read_whatever_the_scope() {
    let dir = konoma_test_support::unique_tmp("cvi_scope_self");
    let (root, out) = (dir.join("root"), dir.join("out"));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&out).unwrap();
    write_png(&out.join("shown.png"), 3);
    let shown = out.join("shown.png");
    let scope = scoped(&[&root]);
    assert!(matches!(
        canvas_file(&shown, &shown, &scope),
        CanvasDecoded::Still(_)
    ));
}

#[test]
fn a_canvas_picture_link_out_is_refused_and_the_scope_does_not_touch_a_standard_plugin() {
    let dir = konoma_test_support::unique_tmp("cvi_scope_link");
    let (root, out) = (dir.join("root"), dir.join("out"));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&out).unwrap();
    write_png(&out.join("out.png"), 4);
    let link = root.join("l.png");
    std::os::unix::fs::symlink(out.join("out.png"), &link).unwrap();
    let shown = root.join("a.spr");
    assert!(matches!(
        canvas_file(&link, &shown, &scoped(&[&root])),
        CanvasDecoded::Failed(konoma_svg::image::ImageFailure::OutsideReadDirs)
    ));
    // Unrestricted (a standard plugin): the same link is followed, as ever.
    assert!(matches!(
        canvas_file(&link, &shown, &crate::ReadScope::unrestricted()),
        CanvasDecoded::Still(_)
    ));
}

#[test]
fn a_canvas_picture_naming_a_sibling_with_the_same_start_is_outside() {
    let dir = konoma_test_support::unique_tmp("cvi_scope_prefix");
    let (b, bc) = (dir.join("b"), dir.join("bc"));
    std::fs::create_dir_all(&b).unwrap();
    std::fs::create_dir_all(&bc).unwrap();
    write_png(&bc.join("p.png"), 5);
    assert!(matches!(
        canvas_file(&bc.join("p.png"), &b.join("a.spr"), &scoped(&[&b])),
        CanvasDecoded::Failed(konoma_svg::image::ImageFailure::OutsideReadDirs)
    ));
}

#[test]
fn a_canvas_frame_list_is_judged_frame_by_frame() {
    let dir = konoma_test_support::unique_tmp("cvi_scope_frames");
    let (root, out) = (dir.join("root"), dir.join("out"));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&out).unwrap();
    write_png(&root.join("a.png"), 6);
    write_png(&out.join("b.png"), 7);
    let frames = ImageData::Frames(vec![
        (
            ImageData::File(root.join("a.png")),
            Duration::from_millis(100),
        ),
        (
            ImageData::File(out.join("b.png")),
            Duration::from_millis(100),
        ),
    ]);
    let c = canvas_of(vec![im(1, frames)]);
    let got = dc(
        "pr",
        &c,
        &nothing_held(),
        &root.join("a.spr"),
        &scoped(&[&root]),
    );
    assert!(matches!(
        got.fresh[0].1,
        CanvasDecoded::Failed(konoma_svg::image::ImageFailure::OutsideReadDirs)
    ));
}

#[test]
fn the_header_pixels_of_every_still_kind_come_from_the_header_alone() {
    for kind in KINDS {
        // 140,000,000 pixels in a file of a few dozen bytes. (WebP lossless sides stop at 16384.)
        let (w, h) = match kind {
            Kind::Webp => (16_000, 9_000),
            _ => (14_000, 10_000),
        };
        let bytes = huge_header(kind, w, h, 1);
        assert!(
            bytes.len() < 1000,
            "{kind:?}: a tiny file ({} bytes)",
            bytes.len()
        );
        assert_eq!(
            header_pixels(
                &encoded(bytes),
                Path::new("/x/a.spr"),
                &crate::ReadScope::unrestricted()
            ),
            (u64::from(w) * u64::from(h), false),
            "{kind:?}"
        );
    }
    // Something that is not a picture costs nothing (the decoder refuses it cheaply); a ready
    // `Rgba` costs nothing (a copy).
    assert_eq!(
        header_pixels(
            &encoded(vec![7; 100]),
            Path::new("/x"),
            &crate::ReadScope::unrestricted()
        ),
        (0, false)
    );
    assert_eq!(
        header_pixels(
            &rgba(50, 50, 1),
            Path::new("/x"),
            &crate::ReadScope::unrestricted()
        ),
        (0, false)
    );
}

#[test]
fn a_gif_costs_its_screen_times_its_frames() {
    // Two image blocks on a 100 x 50 screen: a minimal animation, counted from the block structure.
    let mut g = b"GIF89a".to_vec();
    g.extend_from_slice(&[100, 0, 50, 0, 0, 0, 0]);
    for _ in 0..2 {
        // image descriptor (0,0,1,1), no local table, LZW min size 2, one data block, terminator
        g.extend_from_slice(&[0x2C, 0, 0, 0, 0, 1, 0, 1, 0, 0, 2, 2, 0x4C, 0x01, 0]);
    }
    g.push(0x3B);
    assert_eq!(
        header_pixels(
            &encoded(g),
            Path::new("/x"),
            &crate::ReadScope::unrestricted()
        ),
        (100 * 50 * 2, false)
    );
}

#[test]
fn a_tiny_file_with_a_huge_header_waits_before_it_is_decoded() {
    for kind in KINDS {
        let (w, h) = match kind {
            Kind::Webp => (16_000, 9_000),
            _ => (14_000, 10_000),
        };
        let huge = encoded(huge_header(kind, w, h, 3));
        // A first picture that is cheap runs; the huge one does not fit next to it: it waits, and
        // nothing was decoded for it (the decode of the huge header is what the budget protects).
        let c = canvas_of(vec![im(1, rgba(4, 4, 9)), im(2, huge.clone())]);
        let got = decode_within(&c, &nothing_held(), small(|l| l.header_pixels = 1_000_000));
        assert_eq!(got.fresh.len(), 1, "{kind:?}: only the cheap one ran");
        assert_eq!(got.waiting, 1, "{kind:?}");
        assert!(got.more, "{kind:?}: asked again");
        assert_eq!(
            decoded_times(&huge),
            0,
            "{kind:?}: the huge header was not decoded"
        );
        // Not failed and not refused for good: the host kept its source for the next answer.
        assert!(got.sources.iter().any(|(id, _)| *id == 2), "{kind:?}");
        // Alone in an answer it is the first picture and runs (an answer makes progress).
        let alone = decode_within(
            &canvas_of(vec![im(2, huge.clone())]),
            &nothing_held(),
            small(|l| l.header_pixels = 1_000_000),
        );
        assert_eq!(alone.waiting, 0, "{kind:?}");
        assert_eq!(alone.fresh.len(), 1, "{kind:?}");
        assert_eq!(
            decoded_times(&huge),
            1,
            "{kind:?}: the first picture is decoded"
        );
    }
}

#[test]
fn the_header_budget_is_the_sum_of_the_pictures_one_answer_started() {
    // Three pictures of 140,000,000 header pixels against the production 400,000,000: the first two
    // start (280,000,000), the third would make 420,000,000 and waits. Nothing here is a valid picture,
    // the decoder fails them cheaply; what is asserted is how many were started.
    let pics: Vec<ImageData> = (111..=113u8)
        .map(|n| encoded(huge_header(Kind::Png, 14_000, 10_000, n)))
        .collect();
    let c = canvas_of(
        pics.iter()
            .enumerate()
            .map(|(i, d)| im(i as u64 + 1, d.clone()))
            .collect(),
    );
    let got = decode("pw", &c, &nothing_held());
    assert_eq!(got.fresh.len(), 2);
    assert_eq!(got.waiting, 1);
    assert!(got.more);
    assert_eq!(decoded_times(&pics[0]), 1);
    assert_eq!(decoded_times(&pics[1]), 1);
    assert_eq!(decoded_times(&pics[2]), 0, "the third waits");
    // What waited is first in line next answer and runs.
    let mut held = held_after(&nothing_held(), &got);
    for (id, k) in &got.sources {
        held.kept.insert(*id, k.clone());
    }
    let next = decode("pw", &c, &held);
    assert_eq!(decoded_times(&pics[2]), 1);
    assert_eq!(next.waiting, 0);
}

#[test]
fn the_header_budget_does_not_touch_an_ordinary_answer() {
    // 32 pictures of 64 x 64 (the most one answer decodes): every one runs, none waits. The
    // pinned numbers: 32 x 4,096 header pixels is far under 400,000,000.
    let pics: Vec<CanvasImage> = (0..32u8)
        .map(|n| im(u64::from(n) + 1, rgba(64, 64, n)))
        .collect();
    let got = decode("pw", &canvas_of(pics), &nothing_held());
    assert_eq!(got.fresh.len(), 32);
    assert_eq!(got.waiting, 0);
    // And the same answer with real encoded pictures.
    let encoded_pics: Vec<CanvasImage> = (0..32u8)
        .map(|n| {
            let mut out = Vec::new();
            image::ImageEncoder::write_image(
                image::codecs::png::PngEncoder::new(&mut out),
                &vec![n; 64 * 64 * 3],
                64,
                64,
                image::ExtendedColorType::Rgb8,
            )
            .unwrap();
            im(u64::from(n) + 100, encoded(out))
        })
        .collect();
    let got = decode("pw", &canvas_of(encoded_pics), &nothing_held());
    assert_eq!(got.fresh.len(), 32);
    assert_eq!(got.waiting, 0);
}

#[test]
fn an_animation_that_alone_costs_more_than_the_header_budget_is_refused_for_good() {
    let frames = ImageData::Frames(
        (1..=3u8)
            .map(|n| {
                (
                    encoded(huge_header(Kind::Png, 1000, 1000, n)),
                    Duration::from_millis(50),
                )
            })
            .collect(),
    );
    let got = decode_within(
        &canvas_of(vec![im(1, frames.clone())]),
        &nothing_held(),
        small(|l| l.header_pixels = 2_000_000),
    );
    assert_eq!(got.waiting, 0, "never decodable: not waiting");
    assert_eq!(got.fresh.len(), 1);
    assert!(
        matches!(
            got.fresh[0].1,
            CanvasDecoded::Failed(konoma_svg::image::ImageFailure::TooLarge)
        ),
        "failed for good, as an animation over the other budgets is"
    );
    assert_eq!(decoded_times(&frames), 0);
}

#[test]
fn a_tiny_file_over_the_per_image_pixel_cap_is_refused_from_its_header_when_it_is_first() {
    // The first picture of an answer always runs, so what keeps a thin file with a huge header
    // from being decoded is the per-image cap (`decode_reader`: `within_decode_limits`, read from
    // the header before the decoder allocates anything). The proof that nothing was decoded: the
    // file holds no pixel data, so a decode that began would fail as `Corrupt`; the refusal is
    // `TooLarge`.
    let dir = konoma_test_support::unique_tmp("cvi_header_cap");
    std::fs::create_dir_all(&dir).unwrap();
    let over = [
        (Kind::Png, 20_000, 20_000),
        (Kind::Jpeg, 30_000, 30_000),
        (Kind::Webp, 16_384, 16_384),
        (Kind::Gif, 30_000, 30_000),
    ];
    for (n, (kind, w, h)) in over.into_iter().enumerate() {
        assert!(u64::from(w) * u64::from(h) > konoma_svg::image::MAX_IMAGE_PIXELS);
        let bytes = huge_header(kind, w, h, 40 + n as u8);
        let path = dir.join(format!("thin_{kind:?}.bin"));
        std::fs::write(&path, &bytes).unwrap();
        for data in [encoded(bytes.clone()), ImageData::File(path.clone())] {
            let got = decode_within(
                &canvas_of(vec![im(1, data)]),
                &nothing_held(),
                CanvasLimits::PRODUCTION,
            );
            assert_eq!(got.fresh.len(), 1, "{kind:?}: the first picture ran");
            use konoma_svg::image::ImageFailure as F;
            let why = match &got.fresh[0].1 {
                CanvasDecoded::Failed(f) => Some(*f),
                _ => None,
            };
            // The WebP decoder of the image crate refuses a header it cannot start from as damaged
            // before it reports a size; either way it stops before any pixel is produced.
            let ok = why == Some(F::TooLarge) || (kind == Kind::Webp && why == Some(F::Corrupt));
            assert!(ok, "{kind:?}: refused from the header, got {why:?}");
        }
    }
    // The line is the existing pixel cap: a 100-megapixel camera image is under it.
    assert!(konoma_svg::image::dimensions_within_limits((11_600, 8_700)));
}

#[test]
fn a_picture_outside_the_read_scope_costs_no_header_pixels_and_is_never_opened() {
    // A user plugin names a file outside its scope whose header announces 6,000,000 pixels. konoma
    // must not open it even to price it: it costs (0, false), so it does not eat the per-answer
    // header budget that the in-scope pictures after it need.
    let dir = konoma_test_support::unique_tmp("cvi_header_scope");
    let (root, out) = (dir.join("root"), dir.join("out"));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&out).unwrap();
    let huge = out.join("huge.png");
    std::fs::write(&huge, huge_header(Kind::Png, 3000, 2000, 1)).unwrap();
    write_png(&root.join("a.png"), 11);
    write_png(&root.join("b.png"), 12);
    let shown = root.join("a.spr");
    let scope = scoped(&[&root]);
    // Priced directly: refused without a read, while the same file under an open scope is read.
    assert_eq!(
        header_pixels(&ImageData::File(huge.clone()), &shown, &scope),
        (0, false)
    );
    assert_eq!(
        header_pixels(
            &ImageData::File(huge.clone()),
            &shown,
            &crate::ReadScope::unrestricted()
        ),
        (6_000_000, false)
    );
    // Through an answer: 9 + 9 header pixels fit the limit of 20 only if the huge one cost 0.
    let c = canvas_of(vec![
        im(1, ImageData::File(huge)),
        im(2, ImageData::File(root.join("a.png"))),
        im(3, ImageData::File(root.join("b.png"))),
    ]);
    let got = dcl(
        "pr",
        &c,
        &nothing_held(),
        &shown,
        &scope,
        small(|l| l.header_pixels = 20),
    );
    assert_eq!(got.waiting, 0, "the out-of-scope header cost nothing");
    assert_eq!(got.fresh.len(), 3);
    assert!(
        matches!(
            got.fresh[0].1,
            CanvasDecoded::Failed(konoma_svg::image::ImageFailure::OutsideReadDirs)
        ),
        "the out-of-scope picture is refused for its own reason"
    );
    assert!(matches!(got.fresh[1].1, CanvasDecoded::Still(_)));
    assert!(matches!(got.fresh[2].1, CanvasDecoded::Still(_)));
}

#[test]
fn a_picture_outside_the_read_scope_consumes_no_budget_of_the_answer() {
    // A huge file outside the scope (sparse: larger than the input budget below, no disk used)
    // and an animation with such a file among its frames come first; in-scope pictures follow.
    // The refused ones are never opened, so they must not use the decode limit (2) or the input
    // budget (64 KiB), and the in-scope pictures after them all decode in this very answer.
    // The folders and the files are named in CJK (a path is bytes, not ASCII).
    let dir = konoma_test_support::unique_tmp("cvi_scope_budget");
    let (root, out) = (dir.join("作業"), dir.join("外部"));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&out).unwrap();
    let huge = out.join("巨大.png");
    std::fs::File::create(&huge)
        .unwrap()
        .set_len(10 * 1024 * 1024)
        .unwrap();
    write_png(&root.join("一.png"), 21);
    write_png(&root.join("二.png"), 22);
    let shown = root.join("a.spr");
    let scope = scoped(&[&root]);
    let anim = ImageData::Frames(vec![
        (
            ImageData::File(root.join("一.png")),
            Duration::from_millis(50),
        ),
        (ImageData::File(huge.clone()), Duration::from_millis(50)),
    ]);
    let c = canvas_of(vec![
        im(1, ImageData::File(huge)),
        im(2, anim),
        im(3, ImageData::File(root.join("一.png"))),
        im(4, ImageData::File(root.join("二.png"))),
    ]);
    let got = dcl(
        "pr",
        &c,
        &nothing_held(),
        &shown,
        &scope,
        small(|l| {
            l.decodes = 2;
            l.input_bytes = 64 * 1024;
        }),
    );
    let outside = |d: &CanvasDecoded| {
        matches!(
            d,
            CanvasDecoded::Failed(konoma_svg::image::ImageFailure::OutsideReadDirs)
        )
    };
    assert_eq!(got.waiting, 0, "nothing waits behind a refused picture");
    assert!(!got.more, "no further answer is needed");
    assert_eq!(got.fresh.len(), 4);
    assert!(outside(&got.fresh[0].1), "the huge file is refused");
    assert!(outside(&got.fresh[1].1), "so is the animation holding it");
    assert!(matches!(got.fresh[2].1, CanvasDecoded::Still(_)));
    assert!(matches!(got.fresh[3].1, CanvasDecoded::Still(_)));
}

#[test]
fn a_stale_answer_stops_counting_the_frames_of_a_gif() {
    let mut g = b"GIF89a".to_vec();
    g.extend_from_slice(&[100, 0, 50, 0, 0, 0, 0]);
    // A comment extension of 70,000 one-byte sub-blocks: the counter looks at `stale` only every
    // 65,536 sub-blocks, so a superseded answer is noticed inside it.
    g.extend_from_slice(&[0x21, 0xFE]);
    for _ in 0..70_000 {
        g.extend_from_slice(&[1, b'x']);
    }
    g.push(0);
    for _ in 0..3 {
        g.extend_from_slice(&[0x2C, 0, 0, 0, 0, 1, 0, 1, 0, 0, 2, 2, 0x4C, 0x01, 0]);
    }
    g.push(0x3B);
    let (data, shown) = (encoded(g), Path::new("/x"));
    let scope = crate::ReadScope::unrestricted();
    assert_eq!(
        header_pixels_within(&data, shown, &scope, &|| false),
        (100 * 50 * 3, false)
    );
    // Superseded: the count is abandoned (one frame's worth), not finished.
    assert_eq!(
        header_pixels_within(&data, shown, &scope, &|| true),
        (100 * 50, false)
    );
}

#[test]
fn hashing_looks_at_the_cancellation_while_a_large_buffer_is_read() {
    let big = ImageData::Encoded(Arc::from(vec![1u8; HASH_CHUNK * 5 + 1]));
    let questions = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let q = questions.clone();
    let cancel = move || q.fetch_add(1, std::sync::atomic::Ordering::SeqCst) >= 2;
    let mut h = Hasher::new(&cancel);
    assert_eq!(
        h.content(&big),
        None,
        "cancelled in the middle of the buffer"
    );
    assert_eq!(h.bytes, 0, "and nothing is counted as hashed");
    assert!(questions.load(std::sync::atomic::Ordering::SeqCst) <= 4);
    // A cancelled job answers with nothing named and nothing to decode.
    let always = || true;
    let scope = crate::ReadScope::unrestricted();
    let policy = CanvasPolicy::new(&scope, &NoSvg, 800).cancelled(&always);
    let got = decode_canvas(
        "phc",
        &canvas_of(vec![im(1, big)]),
        &nothing_held(),
        Path::new("/x"),
        &policy,
    );
    assert!(got.fresh.is_empty() && got.sources.is_empty());
    assert!(got.places[0].key.is_none());
}

#[test]
fn the_size_of_a_source_is_the_memory_it_holds() {
    assert_eq!(source_bytes(&rgba(10, 10, 1)), 400);
    assert_eq!(
        source_bytes(&ImageData::Encoded(Arc::from(vec![0u8; 7]))),
        7
    );
    assert_eq!(
        source_bytes(&ImageData::Svg {
            data: Arc::from(vec![0u8; 9]),
            base_dir: None,
            trusted: false
        }),
        9
    );
    assert_eq!(source_bytes(&ImageData::File(PathBuf::from("/x"))), 0);
    assert_eq!(
        source_bytes(&ImageData::Frames(vec![
            (rgba(1, 1, 1), Duration::from_millis(10)),
            (rgba(2, 1, 1), Duration::from_millis(10)),
        ])),
        12
    );
}

#[test]
fn the_hash_chunk_is_a_mebibyte_and_the_cancellation_is_looked_at_every_chunk() {
    // The pinned value, and a buffer sized in plain bytes (not by the constant): with a bigger
    // chunk the cancellation would be looked at once, before the buffer was read.
    assert_eq!(HASH_CHUNK, 1 << 20);
    let big = ImageData::Encoded(Arc::from(vec![1u8; 5 * 1024 * 1024 + 1]));
    let questions = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let q = questions.clone();
    let cancel = move || q.fetch_add(1, std::sync::atomic::Ordering::SeqCst) >= 2;
    let mut h = Hasher::new(&cancel);
    assert_eq!(h.content(&big), None, "cancelled on the third chunk");
    assert_eq!(h.bytes, 0);
    assert_eq!(questions.load(std::sync::atomic::Ordering::SeqCst), 3);
}

#[test]
fn the_input_of_a_picture_is_the_bytes_the_decode_reads() {
    let dir = konoma_test_support::unique_tmp("cvi_inputbytes");
    std::fs::create_dir_all(&dir).unwrap();
    let f = dir.join("p.bin");
    std::fs::write(&f, vec![7u8; 600]).unwrap();
    assert_eq!(
        input_bytes(&ImageData::File(f.to_path_buf()), Path::new("/x/a.spr")),
        600
    );
    assert_eq!(
        input_bytes(&ImageData::File(dir.join("missing")), Path::new("/x/a.spr")),
        0
    );
    assert_eq!(input_bytes(&junk(1), Path::new("/x/a.spr")), 100);
    assert_eq!(input_bytes(&rgba(2, 3, 1), Path::new("/x/a.spr")), 24);
    let svg = ImageData::Svg {
        data: Arc::from(vec![b' '; 40]),
        base_dir: None,
        trusted: false,
    };
    assert_eq!(input_bytes(&svg, Path::new("/x/a.spr")), 40);
    let frames = ImageData::Frames(vec![
        (junk(1), Duration::from_millis(30)),
        (rgba(1, 1, 1), Duration::from_millis(30)),
    ]);
    assert_eq!(input_bytes(&frames, Path::new("/x/a.spr")), 104);
}

// ---- draw_plugin_svg: who draws what -------------------------------------------------------------

const TINY_SVG: &[u8] =
    br#"<svg xmlns="http://www.w3.org/2000/svg" width="4" height="4"><rect width="4" height="4"/></svg>"#;

/// Records which `SvgDraw` method was called (and with which directory), and draws a 1x1 image.
#[derive(Default)]
struct RecordingSvg {
    calls: std::cell::RefCell<Vec<String>>,
}

impl SvgDraw for RecordingSvg {
    fn untrusted(
        &self,
        _: &[u8],
        base: &Path,
        roots: Option<&[PathBuf]>,
        _: u32,
        _: &dyn Fn() -> bool,
    ) -> Result<image::DynamicImage, konoma_svg::svg::RasterError> {
        self.calls
            .borrow_mut()
            .push(format!("untrusted:{}:{}", base.display(), roots.is_some()));
        Ok(image::DynamicImage::new_rgba8(1, 1))
    }

    fn embedded(
        &self,
        _: &[u8],
        _: u32,
        _: &dyn Fn() -> bool,
    ) -> Result<image::DynamicImage, konoma_svg::svg::RasterError> {
        self.calls.borrow_mut().push("embedded".into());
        Ok(image::DynamicImage::new_rgba8(1, 1))
    }
}

fn draw_svg(
    base_dir: Option<PathBuf>,
    trusted: bool,
    scope: &crate::ReadScope,
    trust_own_svg: bool,
    rec: &RecordingSvg,
) -> Result<(image::DynamicImage, konoma_widgets::inline_images::SvgTrust), Option<ImageFailure>> {
    draw_plugin_svg(
        TINY_SVG,
        base_dir,
        trusted,
        Path::new("/x/shown.md"),
        64,
        scope,
        trust_own_svg,
        rec,
        &|| false,
    )
}

#[test]
fn an_svg_is_drawn_in_process_only_when_the_plugin_claims_trust_and_the_host_believes_it() {
    use konoma_widgets::inline_images::SvgTrust;
    let scope = crate::ReadScope::unrestricted();
    let rec = RecordingSvg::default();
    let (_, trust) = draw_svg(Some(PathBuf::from("/x")), true, &scope, true, &rec).expect("draws");
    assert!(matches!(trust, SvgTrust::Trusted(_)), "{trust:?}");
    assert!(
        rec.calls.borrow().is_empty(),
        "a trusted SVG is not handed to the drawer"
    );
    // Trusted with no directory too.
    let (_, trust) = draw_svg(None, true, &scope, true, &rec).expect("draws");
    assert!(matches!(trust, SvgTrust::Trusted(_)), "{trust:?}");
    assert!(rec.calls.borrow().is_empty());
}

#[test]
fn an_untrusted_svg_never_draws_in_process() {
    use konoma_widgets::inline_images::SvgTrust;
    let scope = crate::ReadScope::unrestricted();
    // (plugin claims trust, host believes the claim) with at least one false.
    for (claim, believe) in [(false, true), (true, false), (false, false)] {
        let rec = RecordingSvg::default();
        let (_, trust) =
            draw_svg(Some(PathBuf::from("/x")), claim, &scope, believe, &rec).expect("draws");
        assert!(
            matches!(trust, SvgTrust::File(_)),
            "claim={claim} believe={believe}: {trust:?}"
        );
        assert_eq!(
            *rec.calls.borrow(),
            vec!["untrusted:/x/shown.md:false".to_string()],
            "claim={claim} believe={believe}"
        );

        let rec = RecordingSvg::default();
        let (_, trust) = draw_svg(None, claim, &scope, believe, &rec).expect("draws");
        assert!(
            matches!(trust, SvgTrust::Embedded),
            "claim={claim} believe={believe}: {trust:?}"
        );
        assert_eq!(*rec.calls.borrow(), vec!["embedded".to_string()]);
    }
}

#[test]
fn an_untrusted_svg_in_a_restricted_scope_passes_the_roots_to_the_drawer() {
    use konoma_widgets::inline_images::SvgTrust;
    let dir = konoma_test_support::unique_tmp("cvi_svgroots");
    std::fs::create_dir_all(&dir).unwrap();
    let scope = scoped(&[dir.as_path()]);
    let rec = RecordingSvg::default();
    let (_, trust) = draw_svg(
        Some(dir.as_path().to_path_buf()),
        false,
        &scope,
        false,
        &rec,
    )
    .expect("a directory inside the scope draws");
    assert!(matches!(trust, SvgTrust::FileWithin(..)), "{trust:?}");
    let calls = rec.calls.borrow();
    assert_eq!(calls.len(), 1);
    assert!(
        calls[0].starts_with("untrusted:") && calls[0].ends_with(":true"),
        "{calls:?}"
    );
}

#[test]
fn a_base_dir_outside_the_scope_is_refused_before_any_drawing() {
    let inside = konoma_test_support::unique_tmp("cvi_svgin");
    std::fs::create_dir_all(&inside).unwrap();
    let outside = konoma_test_support::unique_tmp("cvi_svgout");
    std::fs::create_dir_all(&outside).unwrap();
    let scope = scoped(&[inside.as_path()]);
    // Whatever the trust: the refusal comes first, so neither the drawer nor the in-process
    // drawing is reached.
    for (claim, believe) in [(false, false), (true, true), (true, false)] {
        let rec = RecordingSvg::default();
        let r = draw_svg(
            Some(outside.as_path().to_path_buf()),
            claim,
            &scope,
            believe,
            &rec,
        );
        assert!(
            matches!(r, Err(Some(ImageFailure::OutsideReadDirs))),
            "claim={claim} believe={believe}"
        );
        assert!(rec.calls.borrow().is_empty());
    }
}

mod host;
