//! Crafted files: nothing panics, everything is bounded, and what comes out is well-formed.

use std::time::{Duration, Instant};

use super::tests::*;
use super::tests_emf as em;
use super::tests_wmf as wm;
use super::*;

/// A file with a bit of every EMF record family (the mutation seed).
fn rich_emf() -> Vec<u8> {
    let mut e = Emf::new(100, 100);
    e.solid(1, em::RED)
        .pen(2, 0, 4, 0)
        .brush(3, 2, em::GREEN, 3)
        .select(1)
        .select(2);
    em::font(&mut e, 4, -14, 100, 700, [1, 0, 0], 0, "Arial");
    e.r(33, &[]);
    e.rect(5, 5, 50, 50)
        .r(44, &[10, 10, 90, 90, 20, 20])
        .r(42, &[20, 20, 80, 80]);
    em::pts32(&mut e, 3, &[(50, 10), (90, 90), (10, 90)]);
    em::pts16(&mut e, 87, &[(5, 5), (95, 5), (95, 95)]);
    em::pts32(&mut e, 2, &[(10, 90), (10, 10), (90, 10), (90, 90)]);
    em::arc_rec(&mut e, 47, (100, 50), (50, 0));
    em::arc_rec(&mut e, 45, (100, 50), (50, 0));
    e.r(41, &[50, 50, 30, fl(10.0), fl(100.0)]);
    e.select(3).r(17, &[8]).r(9, &[10, 10]).r(11, &[100, 100]);
    e.r(35, &[fl(1.0), fl(0.2), fl(-0.2), fl(1.0), fl(3.0), fl(4.0)]);
    e.r(30, &[0, 0, 8, 8]);
    em::triangle_path(&mut e);
    e.r(63, &[0, 0, 0, 0]);
    e.select(4);
    em::text_rec(&mut e, true, 10, 40, "Rich", Some(&[5, 6, 7, 8]), 0);
    em::stretchdib(
        &mut e,
        (0, 0, 8, 8),
        (0, 0, 2, 2),
        &em::bmi(2, 2, 24, 0, &[], &[]),
        &em::bits24(2, &em::quad(), false),
        0x00CC0020,
    );
    e.r(34, &[-1i64]);
    e.finish()
}

fn rich_wmf() -> Vec<u8> {
    let mut m = wm::base(100, 100);
    m.brush(0, wm::RED, 0).pen(0, 4, 0).brush(2, wm::GREEN, 3);
    wm::font(&mut m, -14, 100, 700, 0, "Arial");
    m.select(0).select(1);
    m.r(0x001E, &[]);
    m.rect(5, 5, 50, 50)
        .r(0x0418, &[80, 80, 20, 20])
        .r(0x061C, &[20, 20, 90, 90, 10, 10]);
    wm::pts(&mut m, 0x0324, &[(50, 10), (90, 90), (10, 90)]);
    wm::pts(&mut m, 0x0325, &[(5, 5), (95, 5), (95, 95)]);
    m.r(0x081A, &[0, 50, 50, 100, 100, 100, 0, 0]);
    m.r(0x0416, &[60, 60, 10, 10]);
    m.select(3);
    wm::textout(&mut m, 10, 40, b"Rich");
    wm::dib_record(
        &mut m,
        0x0B41,
        &[2, 2, 0, 0, 20, 20, 0, 0],
        0x00CC0020,
        None,
        &em::bmi(2, 2, 24, 0, &[], &[]),
        &em::bits24(2, &em::quad(), false),
    );
    m.r(0x0127, &[-1]);
    m.finish()
}

/// Deterministic xorshift.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

/// Converts, timing it and checking what comes out.
fn check(bytes: &[u8]) -> Option<MetaSvg> {
    let t = Instant::now();
    let r = to_svg(bytes);
    assert!(
        t.elapsed() < Duration::from_secs(20),
        "took {:?} for {} bytes",
        t.elapsed(),
        bytes.len()
    );
    if let Some(m) = &r {
        assert!(
            !m.svg.contains("NaN") && !m.svg.contains("inf"),
            "non-finite number"
        );
        assert!(
            m.svg.len() < 48 * 1024 * 1024,
            "output of {} bytes",
            m.svg.len()
        );
        assert!(
            m.width_px.is_finite()
                && m.height_px.is_finite()
                && m.width_px > 0.0
                && m.height_px > 0.0
        );
    }
    r
}

fn well_formed(svg: &str) {
    resvg::usvg::roxmltree::Document::parse(svg)
        .unwrap_or_else(|e| panic!("{e}\n{}", &svg[..svg.len().min(2000)]));
}

#[test]
fn the_seeds_are_themselves_fine() {
    for seed in [rich_emf(), rich_wmf()] {
        let m = check(&seed).expect("converts");
        assert!(!m.truncated);
        well_formed(&m.svg);
        raster_svg(&m.svg, 100);
    }
}

#[test]
fn emf_mutation_fuzz() {
    fuzz(&rich_emf(), 0x9E37_79B9_7F4A_7C15, 2500);
}

#[test]
fn wmf_mutation_fuzz() {
    fuzz(&rich_wmf(), 0xD1B5_4A32_D192_ED03, 2500);
}

fn fuzz(seed: &[u8], state: u64, rounds: usize) {
    let mut rng = Rng(state);
    for i in 0..rounds {
        let mut b = seed.to_vec();
        match rng.below(5) {
            0 => {
                for _ in 0..1 + rng.below(8) {
                    let at = rng.below(b.len());
                    b[at] = rng.next() as u8;
                }
            }
            1 => b.truncate(rng.below(b.len())),
            2 => {
                // Overwrite a word with an extreme value.
                let at = rng.below(b.len().saturating_sub(4));
                let v = [0u32, 1, 0x7FFF_FFFF, 0x8000_0000, 0xFFFF_FFFF, 0xFFFF_FFF0][rng.below(6)];
                b[at..at + 4].copy_from_slice(&v.to_le_bytes());
            }
            3 => {
                let at = rng.below(b.len());
                let n = rng.below(64);
                let junk: Vec<u8> = (0..n).map(|_| rng.next() as u8).collect();
                b.splice(at..at, junk);
            }
            _ => {
                let at = rng.below(b.len());
                let n = rng.below(b.len() - at);
                b.drain(at..at + n);
            }
        }
        if let Some(m) = check(&b) {
            if i % 25 == 0 {
                well_formed(&m.svg);
                let _ = crate::preview::svg::rasterize_trusted(
                    m.svg.as_bytes(),
                    std::path::Path::new("/m.svg"),
                    64,
                );
            }
        }
    }
}

#[test]
fn random_bodies_behind_valid_headers() {
    let mut rng = Rng(0x1234_5678_9ABC_DEF1);
    let emf_head = Emf::new(100, 100).finish()[..88].to_vec();
    let wmf_head = wm::base(100, 100).finish();
    let wmf_head = wmf_head[..22 + 18].to_vec();
    for _ in 0..600 {
        for head in [&emf_head, &wmf_head] {
            let mut b = head.clone();
            let n = rng.below(600);
            b.extend((0..n).map(|_| rng.next() as u8));
            check(&b);
        }
    }
}

#[test]
fn tiny_and_truncated_inputs() {
    for n in 0..120usize {
        let e = rich_emf();
        let w = rich_wmf();
        check(&e[..n.min(e.len())]);
        check(&w[..n.min(w.len())]);
    }
}

// ----- record sizes -------------------------------------------------------------------------

fn emf_with_raw_record(size: u32, ty: u32, extra: &[u8]) -> Vec<u8> {
    let mut e = Emf::new(100, 100);
    e.fill_with(em::RED).rect(0, 0, 10, 10);
    let mut b = e.finish();
    let n = b.len();
    // Replace the EOF record by the hostile one.
    b.truncate(n - 20);
    b.extend(ty.to_le_bytes());
    b.extend(size.to_le_bytes());
    b.extend(extra);
    b
}

#[test]
fn a_wrong_record_size_stops_cleanly_and_says_so() {
    for size in [0u32, 1, 4, 7, 9, 12, 0xFFFF_FFFF, 0x7FFF_FFFF, 1_000_000] {
        let b = emf_with_raw_record(size, 43, &[0; 16]);
        let m = check(&b).expect("the part before is drawn");
        assert!(m.truncated, "size {size}");
    }
    // The first, valid part of the file is still drawn.
    let m = check(&emf_with_raw_record(0, 43, &[])).unwrap();
    assert!(is_red(at(&raster_svg(&m.svg, 100), 5, 5)));
}

#[test]
fn a_record_too_short_for_its_fields_is_dropped() {
    for ty in 1..130u32 {
        let b = emf_with_raw_record(12, ty, &[1, 2, 3, 4]);
        check(&b);
    }
}

#[test]
fn every_record_type_with_zeroed_and_with_maximal_payloads() {
    for ty in 1..130u32 {
        for fill in [0u8, 0xFF, 0x7F, 0x80] {
            let mut e = Emf::new(100, 100);
            e.fill_with(em::RED);
            e.raw(ty, &[fill; 200]);
            e.rect(0, 0, 10, 10);
            check(&e.finish());
        }
    }
}

#[test]
fn every_wmf_function_with_zeroed_and_with_maximal_payloads() {
    let mut funcs: Vec<u16> = (0..0x100).collect();
    funcs.extend((0..0x10u16).flat_map(|hi| {
        [
            0x0127, 0x01F0, 0x0214, 0x0521, 0x0A32, 0x0B41, 0x0F43, 0x0940, 0x0538, 0x0324,
        ]
        .map(|f| f | (hi << 8))
    }));
    funcs.extend([
        0x02FA, 0x02FB, 0x02FC, 0x0142, 0x01F9, 0x00F7, 0x06FF, 0x061C, 0x0817, 0x081A, 0x0830,
        0x061D,
    ]);
    for f in funcs {
        if f == 0 {
            continue;
        }
        for fill in [0u8, 0xFF, 0x7F, 0x80] {
            let mut m = wm::base(100, 100);
            m.raw(f, &[fill; 120]);
            m.rect(0, 0, 10, 10);
            check(&m.finish());
        }
    }
}

#[test]
fn a_wmf_record_with_a_wrong_size() {
    let good = rich_wmf();
    let start = 22 + 18;
    for size in [0u32, 1, 2, 3, 5, 0xFFFF, 0x7FFF_FFFF, 0xFFFF_FFFF] {
        let mut b = good.clone();
        b[start..start + 4].copy_from_slice(&size.to_le_bytes());
        check(&b);
    }
}

// ----- volume -------------------------------------------------------------------------------

#[test]
fn a_million_records_is_cut_quickly() {
    let mut e = Emf::new(100, 100);
    for _ in 0..1_000_000 {
        e.recs.extend(18u32.to_le_bytes());
        e.recs.extend(12u32.to_le_bytes());
        e.recs.extend(1u32.to_le_bytes());
    }
    e.count = 1_000_001;
    let t = Instant::now();
    let m = check(&e.finish()).expect("converts");
    assert!(m.truncated);
    assert!(t.elapsed() < Duration::from_secs(10), "{:?}", t.elapsed());
}

#[test]
fn a_million_wmf_records_is_cut_quickly() {
    let mut m = wm::base(100, 100);
    for _ in 0..1_000_000 {
        m.recs.extend(4u32.to_le_bytes());
        m.recs.extend(0x0102u16.to_le_bytes());
        m.recs.extend(1u16.to_le_bytes());
    }
    let r = check(&m.finish()).expect("converts");
    assert!(r.truncated);
}

#[test]
fn a_hundred_thousand_line_segments_are_bounded() {
    let mut e = Emf::new(100, 100);
    e.pen(1, 0, 1, 0).select(1);
    e.r(27, &[0, 0]);
    for i in 0..300_000i64 {
        e.r(54, &[i % 100, (i * 7) % 100]);
    }
    let m = check(&e.finish()).expect("converts");
    assert!(m.svg.len() < 12 * 1024 * 1024);
}

#[test]
fn many_shapes_hit_the_markup_budget() {
    let mut e = Emf::new(100, 100);
    e.fill_with(em::RED);
    for i in 0..200_000i64 {
        e.r(42, &[i % 50, i % 40, 50 + i % 50, 60 + i % 40]);
    }
    let m = check(&e.finish()).expect("converts");
    assert!(m.truncated);
    assert!(m.svg.len() < 12 * 1024 * 1024, "{}", m.svg.len());
}

#[test]
fn huge_point_counts_are_refused() {
    for ty in [2u32, 3, 4, 5, 6, 85, 86, 87, 88, 89, 7, 8, 90, 91, 56, 92] {
        let mut e = Emf::new(100, 100);
        e.fill_with(em::RED);
        e.r(
            ty,
            &[
                0,
                0,
                0,
                0,
                0xFFFF_FFFFu32 as i64,
                0xFFFF_FFFFu32 as i64,
                1,
                2,
                3,
                4,
            ],
        );
        check(&e.finish());
        let mut e = Emf::new(100, 100);
        e.r(ty, &[0, 0, 0, 0, 100_000_000, 100_000_000, 1, 2, 3, 4]);
        check(&e.finish());
    }
}

#[test]
fn polypolygon_counts_that_do_not_add_up() {
    let mut e = Emf::new(100, 100);
    e.fill_with(em::RED);
    // Claims 2 polygons of 1000 and 2000 points in a total of 5: refused.
    e.r(
        8,
        &[0, 0, 0, 0, 2, 5, 1000, 2000, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5],
    );
    // Total larger than the data.
    e.r(8, &[0, 0, 0, 0, 1, 50000, 50000, 1, 1]);
    check(&e.finish());
}

#[test]
fn the_object_table_is_bounded() {
    let mut e = Emf::new(100, 100);
    for i in 1..200_000u32 {
        e.solid(i, em::RED);
    }
    e.select(5).rect(0, 0, 10, 10);
    let m = check(&e.finish()).expect("converts");
    assert!(m.truncated);

    let mut w = wm::base(100, 100);
    for _ in 0..200_000 {
        w.brush(0, wm::RED, 0);
    }
    w.select(3).rect(0, 0, 10, 10);
    let m = check(&w.finish()).expect("converts");
    assert!(m.truncated);
}

#[test]
fn object_indexes_at_the_extremes() {
    let mut e = Emf::new(100, 100);
    for idx in [0u32, 1, 0x7FFF_FFFF, 0x8000_0000, 0x8000_0013, 0xFFFF_FFFF] {
        e.solid(idx, em::RED);
        e.select(idx);
        e.r(40, &[idx as i64]);
    }
    e.rect(0, 0, 10, 10);
    check(&e.finish());
    let mut w = wm::base(100, 100);
    w.select(0x7FFF).select(-32768).delete(0x7FFF).delete(-1);
    w.rect(0, 0, 10, 10);
    check(&w.finish());
}

#[test]
fn deeply_nested_save_dc_is_bounded() {
    let mut e = Emf::new(100, 100);
    e.fill_with(em::RED);
    for _ in 0..200_000 {
        e.r(33, &[]);
    }
    for _ in 0..200_000 {
        e.r(34, &[-1i64]);
    }
    e.rect(0, 0, 10, 10);
    let m = check(&e.finish()).expect("converts");
    assert!(m.truncated);
    let mut w = wm::base(100, 100);
    for _ in 0..100_000 {
        w.r(0x001E, &[]);
    }
    check(&w.finish());
}

#[test]
fn clip_spam_is_bounded() {
    let mut e = Emf::new(100, 100);
    e.fill_with(em::RED);
    for i in 0..300_000i64 {
        e.r(30, &[0, 0, 100 - i % 3, 100]);
        if i % 100 == 0 {
            e.rect(0, 0, 10, 10);
        }
    }
    let m = check(&e.finish()).expect("converts");
    assert!(m.svg.matches("<g clip-path").count() < 100_000);
}

#[test]
fn path_spam_is_bounded() {
    let mut e = Emf::new(100, 100);
    e.fill_with(em::RED);
    e.r(59, &[]);
    for i in 0..400_000i64 {
        e.r(43, &[i % 90, i % 80, i % 90 + 10, i % 80 + 10]);
    }
    e.r(60, &[]).r(62, &[0, 0, 0, 0]);
    let m = check(&e.finish()).expect("converts");
    assert!(m.svg.len() < 12 * 1024 * 1024);
}

// ----- numbers ------------------------------------------------------------------------------

#[test]
fn extreme_coordinates_stay_finite() {
    let ext = [
        0i64,
        1,
        -1,
        i32::MAX as i64,
        i32::MIN as i64,
        1 << 20,
        -(1 << 20),
    ];
    for &a in &ext {
        for &b in &ext {
            let mut e = Emf::new(100, 100);
            e.fill_with(em::RED);
            e.r(43, &[a, b, b, a]);
            e.r(42, &[a, b, b, a]);
            e.r(44, &[a, b, b, a, a, b]);
            e.r(47, &[a, b, b, a, b, a, a, b]);
            e.r(54, &[a, b]);
            e.r(41, &[a, b, a, fl(1.0e30), fl(f32::NAN)]);
            let m = check(&e.finish()).expect("converts");
            raster_svg(&m.svg, 64);
        }
    }
}

#[test]
fn extreme_mappings_stay_finite() {
    let ext = [0i64, 1, -1, i32::MAX as i64, i32::MIN as i64, 3];
    for &a in &ext {
        for &b in &ext {
            for mode in [1i64, 2, 3, 6, 7, 8, 9, 0] {
                let mut e = Emf::new(100, 100);
                e.fill_with(em::RED);
                e.r(17, &[mode])
                    .r(9, &[a, b])
                    .r(11, &[b, a])
                    .r(10, &[a, a])
                    .r(12, &[b, b]);
                e.r(31, &[a, b, b, a]).r(32, &[b, a, a, b]);
                e.rect(0, 0, 50, 50);
                let m = check(&e.finish()).expect("converts");
                raster_svg(&m.svg, 64);
            }
        }
    }
}

#[test]
fn extreme_world_transforms() {
    let vals = [
        0.0f32,
        1.0,
        -1.0,
        1.0e30,
        -1.0e30,
        f32::NAN,
        f32::INFINITY,
        f32::MIN_POSITIVE,
        1.0e-30,
    ];
    for &a in &vals {
        for &b in &vals {
            let mut e = Emf::new(100, 100);
            e.fill_with(em::RED);
            e.r(35, &[fl(a), fl(b), fl(b), fl(a), fl(b), fl(a)]);
            e.r(36, &[fl(b), fl(a), fl(a), fl(b), fl(a), fl(b), 2]);
            e.r(36, &[fl(b), fl(a), fl(a), fl(b), fl(a), fl(b), 3]);
            e.rect(0, 0, 50, 50);
            em::text_rec(&mut e, true, 5, 5, "x", None, 0);
            let m = check(&e.finish()).expect("converts");
            raster_svg(&m.svg, 64);
        }
    }
}

#[test]
fn huge_pen_widths_and_font_sizes() {
    for w in [0i64, 1, 1 << 30, i32::MAX as i64, i32::MIN as i64] {
        let mut e = Emf::new(100, 100);
        e.pen(1, 0, w as i32, 0).select(1);
        e.raw(
            95,
            &words(&[
                2,
                0,
                0,
                0,
                0,
                0x10000 | 7,
                w,
                0,
                0,
                0,
                16,
                w,
                w,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
            ]),
        );
        e.select(2);
        em::pts32(&mut e, 4, &[(0, 0), (90, 90)]);
        em::font(&mut e, 3, w as i32, w as i32, w as i32, [1; 3], 0, "Arial");
        e.select(3);
        em::text_rec(
            &mut e,
            true,
            10,
            10,
            "big",
            Some(&[w as i32, w as i32, w as i32]),
            0,
        );
        let m = check(&e.finish()).expect("converts");
        raster_svg(&m.svg, 64);
    }
}

#[test]
fn text_fields_pointing_outside_the_record() {
    for (n, off, off_dx) in [
        (5i64, 76i64, 0i64),
        (0xFFFF_FFFF, 76, 0),
        (5, 0xFFFF_FFF0, 0),
        (5, 76, 0xFFFF_FFF0),
        (5, 1 << 30, 1 << 30),
        (100_000, 76, 76),
    ] {
        for ty in [83u32, 84] {
            let mut e = Emf::new(100, 100);
            e.raw(
                ty,
                &words(&[
                    0,
                    0,
                    0,
                    0,
                    1,
                    fl(1.0),
                    fl(1.0),
                    10,
                    10,
                    n,
                    off,
                    0x2000,
                    0,
                    0,
                    0,
                    0,
                    off_dx,
                    0x41,
                    0x42,
                ]),
            );
            check(&e.finish());
        }
    }
}

#[test]
fn bitmap_fields_pointing_outside_the_record() {
    let bmi = em::bmi(2, 2, 24, 0, &[], &[]);
    for ty in [76u32, 77, 80, 81, 114, 116] {
        for bad in [0i64, 1 << 30, 0xFFFF_FFFF, 8, 100] {
            let mut e = Emf::new(100, 100);
            let mut p = words(&[bad; 24]);
            p.extend(&bmi);
            e.raw(ty, &p);
            check(&e.finish());
        }
    }
}

#[test]
fn hostile_dib_headers() {
    let good = em::bmi(2, 2, 24, 0, &[], &[]);
    for at in 0..40usize {
        for v in [0u8, 1, 0x7F, 0x80, 0xFF] {
            let mut b = good.clone();
            b[at] = v;
            let mut e = Emf::new(100, 100);
            em::stretchdib(
                &mut e,
                (0, 0, 10, 10),
                (0, 0, 2, 2),
                &b,
                &em::bits24(2, &em::quad(), false),
                0x00CC0020,
            );
            check(&e.finish());
        }
    }
    for compression in 0..8u32 {
        for bpp in [0u16, 1, 2, 4, 8, 15, 16, 24, 32, 64] {
            let mut e = Emf::new(100, 100);
            em::stretchdib(
                &mut e,
                (0, 0, 10, 10),
                (0, 0, 2, 2),
                &em::bmi(
                    3,
                    3,
                    bpp,
                    compression,
                    &[[1, 2, 3]],
                    &[0xFF, 0xFF00, 0xFF0000],
                ),
                &[0x55; 80],
                0x00CC0020,
            );
            check(&e.finish());
        }
    }
}

#[test]
fn rle_streams_cannot_overrun() {
    let pal = [[1, 2, 3]; 16];
    let streams: Vec<Vec<u8>> = vec![
        vec![0, 2, 255, 255, 0, 1],
        vec![255, 1, 255, 1, 255, 1, 0, 0, 0, 0, 0, 0],
        vec![0, 255, 1, 2, 3],
        vec![0, 3, 1, 2, 3, 0, 1],
        [0, 2, 200, 200].repeat(5000),
        vec![9; 5000],
    ];
    for s in streams {
        for (bpp, comp) in [(8u16, 1u32), (4, 2)] {
            let mut e = Emf::new(100, 100);
            em::stretchdib(
                &mut e,
                (0, 0, 10, 10),
                (0, 0, 4, 4),
                &em::bmi(4, 4, bpp, comp, &pal, &[]),
                &s,
                0x00CC0020,
            );
            check(&e.finish());
        }
    }
}

#[test]
fn an_embedded_png_that_lies_about_its_size() {
    let mut png = Vec::new();
    image::RgbaImage::from_pixel(4, 4, image::Rgba([1, 2, 3, 255]))
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();
    // Declare a huge picture around a tiny PNG, and garbage around a real header.
    for (w, h) in [(4, 4), (30000, 30000), (1, 1)] {
        let mut b = em::bmi(w, h, 0, 5, &[], &[]);
        b.truncate(40);
        let mut e = Emf::new(100, 100);
        em::stretchdib(&mut e, (0, 0, 10, 10), (0, 0, 4, 4), &b, &png, 0x00CC0020);
        em::stretchdib(
            &mut e,
            (0, 0, 10, 10),
            (0, 0, 4, 4),
            &b,
            &png[..20],
            0x00CC0020,
        );
        em::stretchdib(
            &mut e,
            (0, 0, 10, 10),
            (0, 0, 4, 4),
            &b,
            b"garbage that is not an image",
            0x00CC0020,
        );
        check(&e.finish());
    }
    // A PNG whose header claims 100000 x 100000.
    let mut big = png.clone();
    big[16..20].copy_from_slice(&100_000u32.to_be_bytes());
    big[20..24].copy_from_slice(&100_000u32.to_be_bytes());
    let mut b = em::bmi(4, 4, 0, 5, &[], &[]);
    b.truncate(40);
    let mut e = Emf::new(100, 100);
    em::stretchdib(&mut e, (0, 0, 10, 10), (0, 0, 4, 4), &b, &big, 0x00CC0020);
    let m = check(&e.finish()).expect("converts");
    assert!(!m.svg.contains("<image"), "refused by the size limit");
}

#[test]
fn pixel_budget_across_many_bitmaps() {
    // 100 bitmaps of 2000 x 2000 (4 MP each): the 64 MP budget stops them.
    let b = em::bmi(2000, 2000, 1, 0, &[[0, 0, 0], [255, 255, 255]], &[]);
    let bits = vec![0xAAu8; 2000 / 8 * 2000];
    let mut e = Emf::new(100, 100);
    for _ in 0..100 {
        em::stretchdib(
            &mut e,
            (0, 0, 10, 10),
            (0, 0, 2000, 2000),
            &b,
            &bits,
            0x00CC0020,
        );
    }
    let t = Instant::now();
    let m = check(&e.finish()).expect("converts");
    assert!(m.truncated || m.svg.matches("<image").count() < 100);
    assert!(t.elapsed() < Duration::from_secs(40));
}

#[test]
fn recursion_like_structures_do_not_overflow_the_stack() {
    // Clips nested through save/restore cycles, paths inside paths.
    let mut e = Emf::new(100, 100);
    e.fill_with(em::RED);
    for _ in 0..50_000 {
        e.r(59, &[])
            .r(59, &[])
            .r(33, &[])
            .r(30, &[0, 0, 50, 50])
            .r(60, &[])
            .r(67, &[1]);
    }
    e.rect(0, 0, 10, 10);
    check(&e.finish());
}

#[test]
fn every_outcome_is_well_formed_xml() {
    let mut rng = Rng(77);
    let seed = rich_emf();
    for _ in 0..300 {
        let mut b = seed.clone();
        for _ in 0..4 {
            let at = 88 + rng.below(b.len() - 88);
            b[at] = rng.next() as u8;
        }
        if let Some(m) = check(&b) {
            well_formed(&m.svg);
        }
    }
}
