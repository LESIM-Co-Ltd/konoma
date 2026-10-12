//! Tests added from mutation testing of `dib.rs`: header validation, where the pixel data starts
//! in a packed DIB, and the limits applied before anything is allocated.

use super::dib::{self, Opts, MAX_BITMAP_PIXELS};
use super::tests::words;
use super::tests_emf::bmi;
use crate::preview::image::MAX_IMAGE_SIDE;

fn decode(bmi: &[u8], bits: &[u8]) -> Option<dib::Bitmap> {
    let mut budget = 1u64 << 30;
    dib::decode(bmi, bits, &mut budget, Opts::default())
}

fn split(d: &[u8]) -> Option<(usize, usize)> {
    dib::split_packed(d).map(|(h, b)| (h.len(), b.len()))
}

/// A header of `size` bytes (40 or more) for a `w` x `h` bitmap of `bpp`, uncompressed.
fn big_header(size: usize, w: i32, h: i32, bpp: u16) -> Vec<u8> {
    let mut b = words(&[size as i64, w as i64, h as i64]);
    b.extend(1u16.to_le_bytes());
    b.extend(bpp.to_le_bytes());
    b.resize(size, 0);
    b
}

#[test]
fn header_size_must_fit_and_be_at_least_40() {
    let bits = [0u8; 16];
    // a 108-byte (V4) header is fine
    let v4 = big_header(108, 2, 1, 24);
    assert!(decode(&v4, &bits).is_some());
    // 36 bytes cannot hold a BITMAPINFOHEADER even though the buffer is longer
    let mut short = big_header(40, 2, 1, 24);
    short[0..4].copy_from_slice(&36u32.to_le_bytes());
    assert!(decode(&short, &bits).is_none());
    // a header claiming more bytes than there are
    let mut long = big_header(40, 2, 1, 24);
    long[0..4].copy_from_slice(&44u32.to_le_bytes());
    assert!(decode(&long, &bits).is_none());
    // exactly as many as there are is fine
    assert!(decode(&big_header(40, 2, 1, 24), &bits).is_some());
}

#[test]
fn bitmap_side_and_pixel_limits_are_exact() {
    let side = MAX_IMAGE_SIDE as i32;
    // 1 bpp: the widest and the tallest allowed bitmaps decode, one more does not
    let row = |w: i32| ((w as usize).div_ceil(32)) * 4;
    let mono = |w: i32, h: i32| bmi(w, h, 1, 0, &[[0, 0, 0], [255, 255, 255]], &[]);
    let wide = decode(&mono(side, 1), &vec![0; row(side)]).expect("widest");
    assert_eq!((wide.w, wide.h), (MAX_IMAGE_SIDE, 1));
    assert!(decode(&mono(side + 1, 1), &vec![0; row(side + 1)]).is_none());
    let tall = decode(&mono(1, side), &vec![0; row(1) * side as usize]).expect("tallest");
    assert_eq!((tall.w, tall.h), (1, MAX_IMAGE_SIDE));
    assert!(decode(&mono(1, side + 1), &vec![0; row(1) * (side as usize + 1)]).is_none());
    // a core header with a zero side is nothing, whichever side it is
    for (w, h) in [(0u16, 1u16), (1, 0)] {
        let mut core = words(&[12]);
        core.extend(w.to_le_bytes());
        core.extend(h.to_le_bytes());
        core.extend(1u16.to_le_bytes());
        core.extend(24u16.to_le_bytes());
        assert!(decode(&core, &[0; 64]).is_none(), "{w}x{h}");
    }
}

#[test]
fn the_per_bitmap_pixel_limit_is_exact() {
    assert_eq!(MAX_BITMAP_PIXELS, 16 * 1024 * 1024);
    let mono = |w: i32, h: i32| bmi(w, h, 1, 0, &[[0, 0, 0], [255, 255, 255]], &[]);
    let bits = |w: usize, h: usize| vec![0u8; w.div_ceil(32) * 4 * h];
    // exactly the limit is allowed (4096 x 4096) ...
    let ok = decode(&mono(4096, 4096), &bits(4096, 4096)).expect("at the limit");
    assert_eq!((ok.w, ok.h), (4096, 4096));
    // ... one row more is not, nor is a 25 Mpx bitmap whose data are all there
    assert!(decode(&mono(4096, 4097), &bits(4096, 4097)).is_none());
    assert!(decode(&mono(5000, 5000), &bits(5000, 5000)).is_none());
}

#[test]
fn the_callers_allowance_is_exact() {
    let b = bmi(64, 64, 1, 0, &[[0, 0, 0], [255, 255, 255]], &[]);
    let bits = vec![0u8; 8 * 64];
    // 64 x 64 = the price of an attempt: an allowance of exactly that is enough, and is used up
    let mut budget = 4096;
    assert!(dib::decode(&b, &bits, &mut budget, Opts::default()).is_some());
    assert_eq!(budget, 0);
    // one pixel more than the allowance: refused (and the attempt is charged)
    let b = bmi(100, 41, 1, 0, &[[0, 0, 0], [255, 255, 255]], &[]);
    let bits = vec![0u8; 16 * 41];
    let mut budget = 4099;
    assert!(dib::decode(&b, &bits, &mut budget, Opts::default()).is_none());
    assert_eq!(budget, 3);
    let mut budget = 4100;
    assert!(dib::decode(&b, &bits, &mut budget, Opts::default()).is_some());
    assert_eq!(budget, 0);
}

#[test]
fn a_packed_dib_needs_a_positive_width_and_a_height() {
    let with = |w: i32, h: i32| {
        let mut d = bmi(w, h, 24, 0, &[], &[]);
        d.extend([0u8; 16]);
        split(&d)
    };
    assert_eq!(with(2, 1), Some((40, 16)));
    assert_eq!(with(2, -1), Some((40, 16)), "negative height = top down");
    assert_eq!(with(0, 1), None);
    assert_eq!(with(-3, 1), None);
    assert_eq!(with(2, 0), None);
}

#[test]
fn packed_dib_headers_say_where_the_bits_start() {
    let m = [0xFF0000u32, 0x00FF00, 0x0000FF, 0xFF00_0000];
    // plain 24 bpp: right behind the 40-byte header
    let mut d = bmi(2, 1, 24, 0, &[], &[]);
    d.extend([0u8; 8]);
    assert_eq!(split(&d), Some((40, 8)));
    // BI_BITFIELDS holds three masks behind the header, BI_ALPHABITFIELDS four
    let mut d = bmi(2, 1, 32, 3, &[], &m[..3]);
    d.extend([0u8; 8]);
    assert_eq!(split(&d), Some((52, 8)));
    let mut d = bmi(2, 1, 32, 6, &[], &m);
    d.extend([0u8; 8]);
    assert_eq!(split(&d), Some((56, 8)));
    // a colour table behind a deep bitmap (biClrUsed): 3 entries of 4 bytes
    let mut d = bmi(3, 1, 24, 0, &[[1, 2, 3], [4, 5, 6], [7, 8, 9]], &[]);
    d.extend([0u8; 12]);
    assert_eq!(split(&d), Some((52, 12)));
    // ... but biClrUsed means nothing for a bitmap without a depth
    let mut d = bmi(3, 1, 0, 0, &[[1, 2, 3], [4, 5, 6], [7, 8, 9]], &[]);
    d.extend([0u8; 12]);
    assert_eq!(
        split(&d),
        Some((40, 12 + 12)),
        "the 12 table bytes are just data here"
    );
    // a core header (12 bytes) has 3-byte table entries: 1 bpp = 2 entries = 6 bytes
    let mut core = words(&[12]);
    core.extend([2u8, 0, 1, 0, 1, 0, 1, 0]); // w 2, h 1, planes 1, bpp 1
    core.extend([0u8; 6]);
    core.extend([0u8; 4]);
    assert_eq!(split(&core), Some((18, 4)));
}

#[test]
fn a_missing_colour_table_is_told_from_a_short_bitmap() {
    let pal: Vec<[u8; 3]> = (0..=255).map(|i| [i as u8, 0, 0]).collect();
    // 8 bpp, 4 x 2: 8 bytes of pixels. Header + table + pixels: split behind the table.
    let mut d = bmi(4, 2, 8, 0, &pal, &[]);
    let head = 40;
    assert_eq!(d.len(), head + 1024);
    d.extend([0u8; 8]);
    assert_eq!(split(&d), Some((head + 1024, 8)));
    // Header + exactly the pixels: the table was left out
    let d = {
        let mut h = bmi(4, 2, 8, 0, &pal, &[]);
        h.truncate(head);
        h.extend([0u8; 8]);
        h
    };
    assert_eq!(split(&d), Some((head, 8)));
    // Header + too few pixels and no table: nothing sensible
    let d = {
        let mut h = bmi(4, 2, 8, 0, &pal, &[]);
        h.truncate(head);
        h.extend([0u8; 5]);
        h
    };
    assert_eq!(split(&d), None);
    // One byte short of table + pixels: it is read as table-less with a long run of pixel data
    let d = {
        let mut h = bmi(4, 2, 8, 0, &pal, &[]);
        h.extend([0u8; 7]);
        h
    };
    assert_eq!(split(&d), Some((head, 1024 + 7)));
    // Table + pixels exactly: not table-less
    let d = {
        let mut h = bmi(4, 2, 8, 0, &pal, &[]);
        h.extend([0u8; 8]);
        h
    };
    assert_eq!(split(&d), Some((head + 1024, 8)));
    // A compressed bitmap with a table and no data: the bits are empty, not missing
    let two = [[0u8, 0, 0], [255, 255, 255]];
    let d = bmi(4, 2, 8, 1, &two, &[]);
    assert_eq!(d.len(), 48);
    assert_eq!(split(&d), Some((48, 0)));
    // ... and one byte shorter than its table is nothing
    assert_eq!(split(&d[..47]), None);
}

fn rgba(b: &dib::Bitmap) -> Vec<u8> {
    match &b.data {
        dib::BitmapData::Rgba(d) => d.clone(),
        dib::BitmapData::Encoded(..) => panic!("encoded"),
    }
}

fn decode_opts(bmi: &[u8], bits: &[u8], opts: Opts) -> Option<dib::Bitmap> {
    let mut budget = 1u64 << 30;
    dib::decode(bmi, bits, &mut budget, opts)
}

/// Palette whose red channel is `10 * index`.
fn reds(n: usize) -> Vec<[u8; 3]> {
    (0..n).map(|i| [(i * 10) as u8, 0, 0]).collect()
}

#[test]
fn rle_runs_stay_inside_the_bitmap() {
    // 2 x 2, bottom row first: a run of 3 overshoots the row (the third pixel is dropped), EOL, then 2 of index 2
    let b = bmi(2, 2, 8, 1, &reds(3), &[]);
    let d = decode(&b, &[3, 1, 0, 0, 2, 2, 0, 1]).unwrap();
    let want = [
        [20, 0, 0, 255],
        [20, 0, 0, 255],
        [10, 0, 0, 255],
        [10, 0, 0, 255],
    ];
    assert_eq!(rgba(&d), want.concat());
    // a run after the last row is dropped, not drawn over the last row (or off the end)
    let b1 = bmi(1, 1, 8, 1, &reds(3), &[]);
    let d = decode(&b1, &[1, 1, 0, 0, 1, 2, 0, 1]).unwrap();
    assert_eq!(rgba(&d), [10, 0, 0, 255]);
    // an odd trailing byte is not a pair
    assert!(decode(&bmi(1, 1, 8, 1, &reds(3), &[]), &[1, 1, 7]).is_some());
}

#[test]
fn rle_stops_at_the_row_after_the_last() {
    let b = bmi(1, 1, 8, 1, &reds(3), &[]);
    // one row; after its EOL (y = 1) a delta with nothing behind it is still looked at: refused
    assert!(decode(&b, &[1, 2, 0, 0, 0, 2]).is_none());
    // ... but two rows past the end are not read
    assert!(decode(&b, &[1, 2, 0, 0, 0, 0, 0, 2]).is_some());
}

#[test]
fn rle_delta_moves_right_and_down() {
    let b = bmi(3, 2, 8, 1, &reds(3), &[]);
    let d = decode(&b, &[1, 1, 0, 2, 1, 1, 1, 2, 0, 1]).unwrap();
    let t = [0, 0, 0, 0];
    // top row: only the pixel the delta reached; bottom row: the first pixel
    let want = [t, t, [20, 0, 0, 255], [10, 0, 0, 255], t, t];
    assert_eq!(rgba(&d), want.concat());
}

#[test]
fn rle_absolute_runs_are_padded_to_words() {
    // RLE8: three literal pixels, one pad byte, then one more
    let b = bmi(4, 1, 8, 1, &reds(5), &[]);
    let d = decode(&b, &[0, 3, 1, 2, 3, 0, 1, 4, 0, 1]).unwrap();
    let r: Vec<u8> = rgba(&d).chunks(4).map(|p| p[0]).collect();
    assert_eq!(r, [10, 20, 30, 40]);
    // RLE4: five literal nibbles = three bytes + one pad byte
    let b = bmi(5, 1, 4, 2, &reds(16), &[]);
    let d = decode(&b, &[0, 5, 0x12, 0x34, 0x50, 0, 0, 1]).unwrap();
    let r: Vec<u8> = rgba(&d).chunks(4).map(|p| p[0]).collect();
    assert_eq!(r, [10, 20, 30, 40, 50]);
    // RLE4 encoded run: the two nibbles alternate
    let d = decode(&b, &[5, 0x12, 0, 1]).unwrap();
    let r: Vec<u8> = rgba(&d).chunks(4).map(|p| p[0]).collect();
    assert_eq!(r, [10, 20, 10, 20, 10]);
    // an absolute run that runs out of data is refused
    assert!(decode(&b, &[0, 5, 0x12, 0x34]).is_none());
}

#[test]
fn rle_pixel_allowance_is_128_per_byte_exactly() {
    let b = |w: i32| bmi(w, 1, 8, 1, &reds(2), &[]);
    // two bytes of data may promise 256 pixels, not 257
    assert!(decode(&b(256), &[0, 1]).is_some());
    assert!(decode(&b(257), &[0, 1]).is_none());
}

#[test]
fn bitmaps_may_carry_more_bytes_than_they_need() {
    let b = bmi(1, 1, 24, 0, &[], &[]);
    let d = decode(&b, &[1, 2, 3, 0, 9, 9, 9, 9]).unwrap();
    assert_eq!(rgba(&d), [3, 2, 1, 255]);
}

#[test]
fn sixteen_bit_pixels_and_channel_widths() {
    // BI_RGB 16 bpp is X1R5G5B5: red, green, blue side by side (stride 8)
    let b = bmi(3, 1, 16, 0, &[], &[]);
    let bits = [0x00, 0x7C, 0xE0, 0x03, 0x1F, 0x00, 0, 0];
    let d = decode(&b, &bits).unwrap();
    assert_eq!(
        rgba(&d),
        [[255, 0, 0, 255], [0, 255, 0, 255], [0, 0, 255, 255]].concat()
    );
    // 10-bit channels (32 bpp, BI_BITFIELDS): 512 of 1023 is 128 once cut down to 8 bits
    let b = bmi(2, 1, 32, 3, &[], &[0x0000_03FF, 0x000F_FC00, 0x3FF0_0000]);
    let px = |r: u32, g: u32, bl: u32| (r | (g << 10) | (bl << 20)).to_le_bytes();
    let mut bits = px(512, 1023, 0).to_vec();
    bits.extend(px(1023, 0, 512));
    let d = decode(&b, &bits).unwrap();
    assert_eq!(rgba(&d), [[128, 255, 0, 255], [255, 0, 128, 255]].concat());
    // 4-bit channels: 8 of 15 scales to 136
    let b = bmi(1, 1, 16, 3, &[], &[0x0F00, 0x00F0, 0x000F]);
    let d = decode(&b, &[0x08, 0x08, 0, 0]).unwrap();
    assert_eq!(rgba(&d), [136, 0, 136, 255]);
}

#[test]
fn bit_masks_are_read_from_the_header_or_behind_it() {
    // 32 bpp, three masks right behind a 40-byte header: R is the first byte, not the third
    let b = bmi(1, 1, 32, 3, &[], &[0x0000_00FF, 0x0000_FF00, 0x00FF_0000]);
    assert_eq!(
        rgba(&decode(&b, &[10, 20, 30, 40]).unwrap()),
        [10, 20, 30, 255]
    );
    // BI_BITFIELDS with no masks in the buffer at all: the usual BGR layout
    let b = bmi(1, 1, 32, 3, &[], &[]);
    assert_eq!(b.len(), 40);
    assert_eq!(
        rgba(&decode(&b, &[10, 20, 30, 40]).unwrap()),
        [30, 20, 10, 255]
    );
    // a V4 header with BI_RGB has (zero) masks inside it that must not be used
    let v4 = big_header(108, 1, 1, 32);
    assert_eq!(
        rgba(&decode(&v4, &[10, 20, 30, 40]).unwrap()),
        [30, 20, 10, 255]
    );
    // masks do not apply below 16 bpp: a palette bitmap is read by index even with BI_BITFIELDS
    let b = bmi(
        2,
        1,
        8,
        3,
        &[[1, 1, 1], [9, 8, 7]],
        &[0xFF, 0xFF00, 0xFF_0000],
    );
    assert_eq!(
        rgba(&decode(&b, &[1, 0, 0, 0]).unwrap())[..4],
        [9, 8, 7, 255]
    );
    let b = bmi(
        2,
        1,
        8,
        6,
        &[[1, 1, 1], [9, 8, 7]],
        &[0xFF, 0xFF00, 0xFF_0000, 0xFF00_0000],
    );
    assert_eq!(
        rgba(&decode(&b, &[1, 0, 0, 0]).unwrap())[..4],
        [9, 8, 7, 255]
    );
}

#[test]
fn alpha_comes_from_the_fourth_mask_only_when_asked_for() {
    let alpha = Opts {
        alpha: true,
        ..Opts::default()
    };
    let m = [0x0000_00FFu32, 0x0000_FF00, 0x00FF_0000, 0xFF00_0000];
    let px = [10u8, 20, 30, 77];
    // BI_ALPHABITFIELDS: four masks
    let b = bmi(1, 1, 32, 6, &[], &m);
    assert_eq!(
        rgba(&decode_opts(&b, &px, alpha).unwrap()),
        [10, 20, 30, 77]
    );
    assert_eq!(
        rgba(&decode_opts(&b, &px, Opts::default()).unwrap()),
        [10, 20, 30, 255]
    );
    // BI_BITFIELDS with a 56-byte header: the fourth mask is in the header
    let mut v3 = big_header(56, 1, 1, 32);
    v3[16..20].copy_from_slice(&3u32.to_le_bytes());
    for (i, mk) in m.iter().enumerate() {
        v3[40 + i * 4..44 + i * 4].copy_from_slice(&mk.to_le_bytes());
    }
    assert_eq!(
        rgba(&decode_opts(&v3, &px, alpha).unwrap()),
        [10, 20, 30, 77]
    );
    // BI_BITFIELDS with a 40-byte header and three masks: no alpha channel
    let b = bmi(1, 1, 32, 3, &[], &m[..3]);
    assert_eq!(
        rgba(&decode_opts(&b, &px, alpha).unwrap()),
        [10, 20, 30, 255]
    );
}

#[test]
fn premultiplied_alpha_is_undone_for_partly_transparent_pixels_only() {
    let alpha = Opts {
        alpha: true,
        ..Opts::default()
    };
    let b = bmi(3, 1, 32, 0, &[], &[]);
    // BGRA: half transparent (premultiplied 64 -> 128), fully transparent, opaque
    let bits = [16, 32, 64, 128, 1, 2, 3, 0, 5, 6, 7, 255];
    let d = decode_opts(&b, &bits, alpha).unwrap();
    assert_eq!(
        rgba(&d),
        [[128, 64, 32, 128], [3, 2, 1, 0], [7, 6, 5, 255]].concat()
    );
    // not asked for: opaque, bytes as they are
    let d = decode_opts(&b, &bits, Opts::default()).unwrap();
    assert_eq!(rgba(&d)[..4], [64, 32, 16, 255]);
}

#[test]
fn invert_and_colour_key_compare_against_the_stored_colour() {
    let b = bmi(1, 1, 24, 0, &[], &[]);
    let bits = [30, 20, 10, 0]; // stored RGB (10, 20, 30)
    let inv_key = |k: [u8; 3]| Opts {
        invert: true,
        key: Some(k),
        ..Opts::default()
    };
    // inverted colour, transparent because the stored colour is the key
    assert_eq!(
        rgba(&decode_opts(&b, &bits, inv_key([10, 20, 30])).unwrap()),
        [245, 235, 225, 0]
    );
    // the key is not compared against the inverted colour
    assert_eq!(
        rgba(&decode_opts(&b, &bits, inv_key([245, 235, 225])).unwrap()),
        [245, 235, 225, 255]
    );
    // without inversion
    let key = Opts {
        key: Some([10, 20, 30]),
        ..Opts::default()
    };
    assert_eq!(rgba(&decode_opts(&b, &bits, key).unwrap()), [10, 20, 30, 0]);
}

#[test]
fn average_of_the_opaque_pixels_and_crop() {
    let b = bmi(4, 1, 24, 0, &[], &[]);
    let bits = [30, 20, 10, 50, 40, 30, 40, 30, 20, 9, 9, 9, 0, 0, 0, 0];
    let key = Opts {
        key: Some([9, 9, 9]),
        ..Opts::default()
    };
    let d = decode_opts(&b, &bits, key).unwrap();
    // (10,20,30) (30,40,50) (20,30,40) are opaque, the keyed pixel is not
    assert_eq!(d.average(), Some([20, 30, 40]));
    // nothing opaque: no average
    let all = Opts {
        key: Some([0, 0, 0]),
        ..Opts::default()
    };
    let one = decode_opts(&bmi(1, 1, 24, 0, &[], &[]), &[0, 0, 0, 0], all).unwrap();
    assert_eq!(one.average(), None);
    // crop: inside, clamped at both ends, and empty parts
    let c = d.crop(1, 0, 2, 1).unwrap();
    assert_eq!((c.w, c.h), (2, 1));
    assert_eq!(rgba(&c), [30, 40, 50, 255, 20, 30, 40, 255]);
    let c = d.crop(-5, -5, 7, 7).unwrap();
    assert_eq!((c.w, c.h), (2, 1));
    let c = d.crop(3, 0, 10, 10).unwrap();
    assert_eq!((c.w, c.h), (1, 1));
    assert!(d.crop(1, 0, 0, 1).is_none(), "no width");
    assert!(d.crop(1, 0, 1, 0).is_none(), "no height");
    assert!(d.crop(9, 0, 1, 1).is_none(), "outside");
    assert!(d.crop(0, 9, 1, 1).is_none(), "outside");
}

#[test]
fn a_colour_table_missing_from_the_dib_comes_from_the_logical_palette() {
    let fallback = [[1u8, 2, 3], [4, 5, 6]];
    let d = |bmi: &[u8]| {
        let mut budget = 1u64 << 30;
        dib::decode_with(bmi, &[1, 0, 0, 0], &mut budget, Opts::default(), &fallback)
    };
    let full = bmi(2, 1, 8, 0, &[[100, 100, 100], [200, 200, 200]], &[]);
    // the table is there: it wins over the logical palette
    assert_eq!(rgba(&d(&full).unwrap())[..4], [200, 200, 200, 255]);
    // no table bytes at all (the header says 2 colours)
    assert_eq!(rgba(&d(&full[..40]).unwrap())[..4], [4, 5, 6, 255]);
    // a table cut short by one byte counts as missing too
    assert_eq!(rgba(&d(&full[..47]).unwrap())[..4], [4, 5, 6, 255]);
    // behind BITFIELDS masks the table starts after the masks
    let masked = bmi(2, 1, 8, 3, &[[100, 100, 100], [200, 200, 200]], &[1, 2, 3]);
    assert_eq!(rgba(&d(&masked).unwrap())[..4], [200, 200, 200, 255]);
    let masked = bmi(
        2,
        1,
        8,
        6,
        &[[100, 100, 100], [200, 200, 200]],
        &[1, 2, 3, 4],
    );
    assert_eq!(rgba(&d(&masked).unwrap())[..4], [200, 200, 200, 255]);
}

#[test]
fn a_palette_of_indices_into_the_logical_palette() {
    let fallback = [[10u8, 0, 0], [20, 0, 0], [30, 0, 0]];
    let mut b = bmi(2, 1, 8, 0, &[[0, 0, 0], [0, 0, 0]], &[]);
    // the table holds 16-bit indices: 2, then 1
    b[40..44].copy_from_slice(&[2, 0, 1, 0]);
    let mut budget = 1u64 << 30;
    let o = Opts {
        pal_colors: true,
        ..Opts::default()
    };
    let d = dib::decode_with(&b, &[0, 1, 0, 0], &mut budget, o, &fallback).unwrap();
    assert_eq!(rgba(&d), [[30, 0, 0, 255], [20, 0, 0, 255]].concat());
    // an index past the logical palette is black
    b[42..44].copy_from_slice(&[9, 0]);
    let mut budget = 1u64 << 30;
    let d = dib::decode_with(&b, &[0, 1, 0, 0], &mut budget, o, &fallback).unwrap();
    assert_eq!(rgba(&d)[4..8], [0, 0, 0, 255]);
}

fn crc32(data: &[u8]) -> u32 {
    let mut c = 0xFFFF_FFFFu32;
    for &b in data {
        c ^= u32::from(b);
        for _ in 0..8 {
            c = if c & 1 != 0 {
                (c >> 1) ^ 0xEDB8_8320
            } else {
                c >> 1
            };
        }
    }
    !c
}

/// A PNG that is nothing but a signature, an IHDR for `w` x `h` and an empty IDAT: enough to read
/// the size from, and costing nothing to build however large it claims to be.
fn png_header(w: u32, h: u32) -> Vec<u8> {
    let chunk = |ty: &[u8; 4], data: &[u8]| {
        let mut c = (data.len() as u32).to_be_bytes().to_vec();
        let mut body = ty.to_vec();
        body.extend(data);
        c.extend(&body);
        c.extend(crc32(&body).to_be_bytes());
        c
    };
    let mut ihdr = w.to_be_bytes().to_vec();
    ihdr.extend(h.to_be_bytes());
    ihdr.extend([8, 0, 0, 0, 0]);
    let mut png = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    png.extend(chunk(b"IHDR", &ihdr));
    png.extend(chunk(b"IDAT", &[]));
    png
}

#[test]
fn embedded_png_and_jpeg_are_passed_through_and_charged() {
    // the DIB header's own size is irrelevant: 1 x 1
    let png_dib = bmi(1, 1, 0, 5, &[], &[]);
    let png = png_header(64, 64);
    let mut budget = 4096 + 100;
    let d = dib::decode(&png_dib, &png, &mut budget, Opts::default()).expect("png");
    assert_eq!((d.w, d.h, d.top_down), (64, 64, true));
    assert!(matches!(&d.data, dib::BitmapData::Encoded("image/png", b) if *b == png));
    assert_eq!(budget, 100, "charged width x height");
    // exactly the allowance is enough, one pixel less is not (and costs an attempt)
    let mut budget = 4096;
    assert!(dib::decode(&png_dib, &png, &mut budget, Opts::default()).is_some());
    assert_eq!(budget, 0);
    let png = png_header(100, 100);
    let mut budget = 5000;
    assert!(dib::decode(&png_dib, &png, &mut budget, Opts::default()).is_none());
    assert_eq!(budget, 5000 - 4096);
    // over the per-bitmap limit, whatever the allowance (the product, not the sum, of the sides)
    let big = png_header(5000, 5000);
    let mut budget = 1u64 << 40;
    assert!(dib::decode(&png_dib, &big, &mut budget, Opts::default()).is_none());
    let flat = png_header(16 * 1024 * 1024 / 4096, 4096);
    let mut budget = 1u64 << 40;
    let d = dib::decode(&png_dib, &flat, &mut budget, Opts::default()).expect("at the limit");
    assert_eq!(d.w as u64 * d.h as u64, MAX_BITMAP_PIXELS);
    assert_eq!(budget, (1u64 << 40) - MAX_BITMAP_PIXELS);
    // a JPEG
    let mut jpg = Vec::new();
    let img = image::RgbImage::from_pixel(8, 4, image::Rgb([200, 100, 50]));
    image::codecs::jpeg::JpegEncoder::new(&mut jpg)
        .encode_image(&img)
        .unwrap();
    let jpeg_dib = bmi(1, 1, 0, 4, &[], &[]);
    let mut budget = 4096 + 32;
    let d = dib::decode(&jpeg_dib, &jpg, &mut budget, Opts::default()).expect("jpeg");
    assert_eq!((d.w, d.h), (8, 4));
    assert!(matches!(&d.data, dib::BitmapData::Encoded("image/jpeg", _)));
    assert_eq!(budget, 4096);
    // garbage that claims to be a PNG
    let mut budget = 1 << 20;
    assert!(dib::decode(&png_dib, &[1, 2, 3, 4], &mut budget, Opts::default()).is_none());
}
